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
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Select,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
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
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canWrite = can("GUARD_ADMIN");

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
        text: tx(
          "部门编码仅允许小写字母、数字、下划线或连字符（1～64 位）",
          "Department ID must be 1–64 lowercase letters, digits, underscores, or hyphens"
        ),
      });
      return;
    }
    if (!cleanName) {
      setFeedback({
        kind: "err",
        text: tx("部门名称不能为空", "Department name is required"),
      });
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
            ? tx(
                `已创建部门 ${cleanName} (${cleanId})`,
                `Created department ${cleanName} (${cleanId})`
              )
            : tx(
                `已保存部门 ${cleanName} (${cleanId})`,
                `Saved department ${cleanName} (${cleanId})`
              ),
      });
      if (editingMode === "create") {
        setDeptId("");
        setDeptName("");
      }
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setFeedback({
          kind: "err",
          text: tx(
            `层级冲突 [409 ${err.errorCode}]：${err.message}`,
            `Hierarchy conflict [409 ${err.errorCode}]: ${err.message}`
          ),
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
      setFeedback({
        kind: "err",
        text: tx(
          "系统默认根部门不可删除",
          "Default root department cannot be deleted"
        ),
      });
      return;
    }
    setFeedback(null);
    try {
      await axguardApi.deleteDepartment(record.dept_id);
      await onReload();
      setFeedback({
        kind: "ok",
        text: tx(
          `已删除部门 ${record.name} (${record.dept_id})`,
          `Deleted department ${record.name} (${record.dept_id})`
        ),
      });
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setFeedback({
          kind: "err",
          text: tx(
            `无法删除部门 ${record.dept_id}（409 冲突）：仍存在子部门、关联用户或资源（${err.message}）`,
            `Cannot delete ${record.dept_id} (409 conflict): active sub-departments, users, or resources exist (${err.message})`
          ),
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
                {tx("上级", "Parent")}: {node.record.parent_id}
              </span>
            )}
          </div>

          {canWrite && (
            <div className="flex items-center gap-1 shrink-0">
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={() => startCreateChild(node.record.dept_id)}
              >
                <Plus className="h-3 w-3 text-xblue" />
                {tx("新增下级", "Add Child")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                onClick={() => startEdit(node.record)}
              >
                <Edit3 className="h-3 w-3" />
                {tx("编辑", "Edit")}
              </Button>
              {node.record.dept_id !== "default" && (
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  onClick={() => void handleDelete(node.record)}
                  data-testid={`delete-dept-${node.record.dept_id}`}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              )}
            </div>
          )}
        </div>

        {hasChildren && !isCollapsed && (
          <div className="space-y-1.5 ml-3 pl-3 border-l border-border/70">
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
                {tx("组织部门树", "Department Hierarchy")}
              </div>
              <p className="text-[11px] text-muted-foreground">
                {tx(
                  "上级部门自动继承下级部门资源的访问关系。",
                  "Parent departments automatically inherit access to child department resources."
                )}
              </p>
            </div>
            {canWrite && (
              <Button
                variant="outline"
                size="sm"
                type="button"
                onClick={() => startCreateChild(null)}
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建顶级部门", "New Root Dept")}
              </Button>
            )}
          </div>

          {treeRoots.length === 0 ? (
            <EmptyState title={tx("暂无部门数据", "No departments")} />
          ) : (
            <div className="space-y-1.5">
              {treeRoots.map((n) => renderNode(n))}
            </div>
          )}
        </Card>

        <Card className="lg:col-span-5 p-4 space-y-4">
          <div className="border-b border-border pb-3">
            <div className="text-xs font-semibold">
              {editingMode === "create"
                ? tx("新建部门", "New Department")
                : `${tx("编辑部门", "Edit Department")} (${deptId})`}
            </div>
            <div className="text-[11px] text-muted-foreground">
              {tx(
                "部门编码创建后作为主键标识。",
                "Department ID acts as the immutable primary slug."
              )}
            </div>
          </div>

          <form onSubmit={(e) => void handleSave(e)} className="space-y-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("部门编码", "Department ID")} *
              </label>
              <Input
                required
                disabled={!canWrite || editingMode === "edit"}
                placeholder="east_logistics"
                value={deptId}
                onChange={(e) => setDeptId(e.target.value)}
                data-testid="dept-id-input"
                className="h-8 text-xs font-mono-tech"
              />
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("部门名称", "Department Name")} *
              </label>
              <Input
                required
                disabled={!canWrite}
                placeholder={tx("华东物流部", "East Logistics")}
                value={deptName}
                onChange={(e) => setDeptName(e.target.value)}
                data-testid="dept-name-input"
                className="h-8 text-xs"
              />
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("上级部门", "Parent Department")}
              </label>
              <Select
                value={parentId}
                disabled={!canWrite}
                onChange={(e) => setParentId(e.target.value)}
                className="h-8 w-full text-xs"
              >
                <option value="">{tx("无（顶级部门）", "None (Root)")}</option>
                {departments
                  .filter((d) => !forbiddenParentIds.has(d.dept_id))
                  .map((d) => (
                    <option key={d.dept_id} value={d.dept_id}>
                      {d.name} ({d.dept_id})
                    </option>
                  ))}
              </Select>
            </div>

            {canWrite && (
              <div className="flex items-center gap-2 pt-2 border-t border-border">
                <Button
                  type="submit"
                  disabled={saving || !deptId.trim() || !deptName.trim()}
                  data-testid="save-dept-btn"
                >
                  {saving
                    ? tx("保存中…", "Saving…")
                    : editingMode === "create"
                      ? tx("创建部门", "Create Department")
                      : tx("保存变更", "Save Changes")}
                </Button>
                {editingMode === "edit" && (
                  <Button
                    variant="outline"
                    type="button"
                    onClick={() => startCreateChild(null)}
                  >
                    {tx("切换新建", "New Mode")}
                  </Button>
                )}
              </div>
            )}
          </form>
        </Card>
      </div>
    </div>
  );
}
