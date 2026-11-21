import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import {
  KeyRound,
  Lock,
  MonitorSmartphone,
  Plus,
  RefreshCw,
  Search,
  ShieldAlert,
  SlidersHorizontal,
  Trash2,
  Unlock,
  UserPlus,
  X,
} from "lucide-react";
import { ApiError } from "@/lib/api";
import {
  axguardApi,
  type DepartmentRecord,
  type GuardRoleRecord,
  type GuardUserRecord,
  type Sensitivity,
  type SessionSummaryRecord,
  type UserAccountStatus,
} from "@/lib/api/axguard";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { SENSITIVITY_OPTIONS, SENSITIVITY_SHORT_LABELS } from "./constants";
import { isAccountLocked, validateLocalPassword } from "./logic";

interface UsersPanelProps {
  users: GuardUserRecord[];
  roles: GuardRoleRecord[];
  departments: DepartmentRecord[];
  onReload: () => Promise<void>;
}

const ATTR_KEY_PATTERN = /^[a-z][a-z0-9_]{0,62}$/;

function ClearanceBadge({ clearance }: { clearance: Sensitivity }) {
  return (
    <Badge
      variant={
        clearance === "SECRET" || clearance === "CONFIDENTIAL"
          ? "xblue"
          : clearance === "INTERNAL"
            ? "neutral"
            : "outline"
      }
    >
      {SENSITIVITY_SHORT_LABELS[clearance]}
    </Badge>
  );
}

export function UsersPanel({
  users,
  roles,
  departments,
  onReload,
}: UsersPanelProps) {
  const [search, setSearch] = useState("");
  const [deptFilter, setDeptFilter] = useState("");
  const [selectedUserId, setSelectedUserId] = useState<string | null>(null);
  const [banner, setBanner] = useState<{
    kind: "ok" | "err";
    text: string;
  } | null>(null);

  // Create local user modal state
  const [createOpen, setCreateOpen] = useState(false);
  const [newUsername, setNewUsername] = useState("");
  const [newDisplayName, setNewDisplayName] = useState("");
  const [newEmail, setNewEmail] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [newDeptId, setNewDeptId] = useState("default");
  const [newClearance, setNewClearance] = useState<Sensitivity>("INTERNAL");
  const [newRoles, setNewRoles] = useState<string[]>(["VIEWER"]);
  const [creating, setCreating] = useState(false);

  // Edit user drawer state
  const [editDisplayName, setEditDisplayName] = useState("");
  const [editEmail, setEditEmail] = useState("");
  const [editDeptId, setEditDeptId] = useState("default");
  const [editClearance, setEditClearance] = useState<Sensitivity>("INTERNAL");
  const [editStatus, setEditStatus] = useState<UserAccountStatus>("ACTIVE");
  const [editRoles, setEditRoles] = useState<string[]>([]);
  const [savingUser, setSavingUser] = useState(false);

  // Reset password state
  const [resetPasswordVal, setResetPasswordVal] = useState("");
  const [resettingPassword, setResettingPassword] = useState(false);

  // User attributes editor (`sec_user_attributes`)
  const [attrDraft, setAttrDraft] = useState<Record<string, string[]>>({});
  const [newAttrKey, setNewAttrKey] = useState("");
  const [newAttrValuesCsv, setNewAttrValuesCsv] = useState("");
  const [savingAttrs, setSavingAttrs] = useState(false);

  // Active sessions state
  const [sessions, setSessions] = useState<SessionSummaryRecord[]>([]);
  const [loadingSessions, setLoadingSessions] = useState(false);

  const selectedUser = useMemo(
    () => users.find((u) => u.id === selectedUserId) ?? null,
    [users, selectedUserId]
  );

  const loadUserSessions = useCallback(async (userId: string) => {
    setLoadingSessions(true);
    try {
      const list = await axguardApi.listSessions(userId);
      setSessions(list);
    } catch {
      setSessions([]);
    } finally {
      setLoadingSessions(false);
    }
  }, []);

  useEffect(() => {
    if (!selectedUser) {
      return;
    }
    setEditDisplayName(selectedUser.display_name);
    setEditEmail(selectedUser.email);
    setEditDeptId(selectedUser.dept_id);
    setEditClearance(selectedUser.clearance);
    setEditStatus(selectedUser.status);
    setEditRoles([...selectedUser.roles]);
    const clonedAttrs: Record<string, string[]> = {};
    for (const [k, vals] of Object.entries(selectedUser.attributes ?? {})) {
      clonedAttrs[k] = [...vals];
    }
    setAttrDraft(clonedAttrs);
    setResetPasswordVal("");
    void loadUserSessions(selectedUser.id);
  }, [selectedUser, loadUserSessions]);

  const filteredUsers = useMemo(() => {
    const q = search.trim().toLowerCase();
    return users.filter((u) => {
      if (deptFilter && u.dept_id !== deptFilter) {
        return false;
      }
      if (!q) {
        return true;
      }
      return (
        u.username.toLowerCase().includes(q) ||
        u.display_name.toLowerCase().includes(q) ||
        u.email.toLowerCase().includes(q) ||
        u.dept_id.toLowerCase().includes(q) ||
        u.roles.some((r) => r.toLowerCase().includes(q))
      );
    });
  }, [users, search, deptFilter]);

  const roleNameOptions = useMemo(() => {
    const set = new Set<string>(["ADMIN", "ANALYST", "OPERATOR", "VIEWER"]);
    for (const r of roles) {
      set.add(r.name);
    }
    return Array.from(set);
  }, [roles]);

  const deptOptions = useMemo(() => {
    if (departments.length === 0) {
      return [{ tenant_id: "default", dept_id: "default", name: "默认部门", parent_id: null }];
    }
    return departments;
  }, [departments]);

  const handleQuickUnlock = async (user: GuardUserRecord) => {
    setBanner(null);
    try {
      await axguardApi.patchUser(user.id, { unlock: true });
      setBanner({ kind: "ok", text: `已解除账号 ${user.username} 的登录锁定` });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    }
  };

  const handleCreateUser = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const pwdIssue = validateLocalPassword(newPassword, newUsername);
    if (pwdIssue) {
      setBanner({ kind: "err", text: pwdIssue });
      return;
    }
    setCreating(true);
    setBanner(null);
    try {
      const created = await axguardApi.createUser({
        username: newUsername.trim(),
        display_name: newDisplayName.trim() || newUsername.trim(),
        email: newEmail.trim(),
        password: newPassword,
        dept_id: newDeptId,
        clearance: newClearance,
        roles: newRoles,
      });
      setCreateOpen(false);
      setNewUsername("");
      setNewDisplayName("");
      setNewEmail("");
      setNewPassword("");
      setNewRoles(["VIEWER"]);
      setBanner({ kind: "ok", text: `本地账号 ${created.username} 创建成功` });
      await onReload();
      setSelectedUserId(created.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    } finally {
      setCreating(false);
    }
  };

  const handleSaveUser = async () => {
    if (!selectedUser) return;
    setSavingUser(true);
    setBanner(null);
    try {
      await axguardApi.patchUser(selectedUser.id, {
        display_name: editDisplayName.trim(),
        email: editEmail.trim(),
        dept_id: editDeptId,
        clearance: editClearance,
        status: editStatus,
        roles: editRoles,
      });
      setBanner({ kind: "ok", text: `已保存账号 ${selectedUser.username} 的治理配置` });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    } finally {
      setSavingUser(false);
    }
  };

  const handleResetPassword = async () => {
    if (!selectedUser) return;
    const issue = validateLocalPassword(resetPasswordVal, selectedUser.username);
    if (issue) {
      setBanner({ kind: "err", text: issue });
      return;
    }
    setResettingPassword(true);
    setBanner(null);
    try {
      await axguardApi.resetUserPassword(selectedUser.id, resetPasswordVal);
      setResetPasswordVal("");
      setBanner({
        kind: "ok",
        text: `已重置 ${selectedUser.username} 的口令并吊销历史会话`,
      });
      await loadUserSessions(selectedUser.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    } finally {
      setResettingPassword(false);
    }
  };

  const handleAddAttribute = () => {
    const key = newAttrKey.trim();
    if (!ATTR_KEY_PATTERN.test(key)) {
      setBanner({
        kind: "err",
        text: "属性键必须以小写字母开头，仅含小写字母、数字或下划线（如 warehouses）",
      });
      return;
    }
    const vals = newAttrValuesCsv
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    setAttrDraft((prev) => ({
      ...prev,
      [key]: Array.from(new Set([...(prev[key] ?? []), ...vals])),
    }));
    setNewAttrKey("");
    setNewAttrValuesCsv("");
  };

  const handleSaveAttributes = async () => {
    if (!selectedUser) return;
    setSavingAttrs(true);
    setBanner(null);
    try {
      await axguardApi.putUserAttributes(selectedUser.id, attrDraft);
      setBanner({
        kind: "ok",
        text: `已更新 ${selectedUser.username} 的主体扩展属性 (sec_user_attributes)`,
      });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    } finally {
      setSavingAttrs(false);
    }
  };

  const handleRevokeSession = async (familyId: string) => {
    if (!selectedUser) return;
    setBanner(null);
    try {
      await axguardApi.revokeSessionFamily(familyId);
      setBanner({ kind: "ok", text: `已吊销会话族 ${familyId.slice(0, 8)}...` });
      await loadUserSessions(selectedUser.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text: err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err),
      });
    }
  };

  const createPwdHint = newPassword
    ? validateLocalPassword(newPassword, newUsername)
    : null;

  return (
    <div className="space-y-4" data-testid="axguard-users-panel">
      {banner && (
        <div
          role="status"
          className={cn(
            "rounded-md border px-3 py-2 text-xs flex items-center justify-between",
            banner.kind === "ok"
              ? "border-xblue/40 bg-xblue-soft text-foreground"
              : "border-foreground/40 bg-muted text-foreground"
          )}
        >
          <span>{banner.text}</span>
          <button
            type="button"
            onClick={() => setBanner(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2 flex-1 min-w-[240px]">
          <div className="relative flex-1 max-w-xs">
            <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2.5" />
            <Input
              placeholder="搜索用户名、姓名、邮箱、角色..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              data-testid="users-search-input"
              className="pl-8 h-8 text-xs"
            />
          </div>
          <select
            value={deptFilter}
            onChange={(e) => setDeptFilter(e.target.value)}
            className="h-8 rounded-md border border-input bg-background px-2.5 text-xs"
            aria-label="按部门筛选"
          >
            <option value="">全部部门</option>
            {deptOptions.map((d) => (
              <option key={d.dept_id} value={d.dept_id}>
                {d.name} ({d.dept_id})
              </option>
            ))}
          </select>
        </div>

        <div className="flex items-center gap-2">
          <Button variant="outline" type="button" onClick={() => void onReload()}>
            <RefreshCw className="h-3.5 w-3.5" />
            刷新
          </Button>
          <Button
            type="button"
            data-testid="open-create-user-btn"
            onClick={() => setCreateOpen(true)}
          >
            <UserPlus className="h-3.5 w-3.5" />
            新建本地用户
          </Button>
        </div>
      </div>

      <div
        className={cn(
          "grid gap-4",
          selectedUser ? "grid-cols-1 xl:grid-cols-12" : "grid-cols-1"
        )}
      >
        <Card
          className={cn(
            "overflow-hidden",
            selectedUser ? "xl:col-span-7" : "col-span-1"
          )}
        >
          <div className="overflow-x-auto">
            <table className="w-full text-xs border-collapse">
              <thead>
                <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                  <th className="px-3 py-2.5 font-medium">账号 / 姓名</th>
                  <th className="px-3 py-2.5 font-medium">归属部门</th>
                  <th className="px-3 py-2.5 font-medium">安全密级</th>
                  <th className="px-3 py-2.5 font-medium">角色</th>
                  <th className="px-3 py-2.5 font-medium">来源 / 状态</th>
                  <th className="px-3 py-2.5 font-medium text-right">治理操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {filteredUsers.length === 0 ? (
                  <tr>
                    <td
                      colSpan={6}
                      className="px-4 py-8 text-center text-muted-foreground"
                    >
                      未找到匹配的用户账号记录
                    </td>
                  </tr>
                ) : (
                  filteredUsers.map((u) => {
                    const locked = isAccountLocked(u.locked_until);
                    const isSelected = u.id === selectedUserId;
                    return (
                      <tr
                        key={u.id}
                        onClick={() => setSelectedUserId(u.id)}
                        className={cn(
                          "hover:bg-muted/40 transition-colors cursor-pointer",
                          isSelected && "bg-xblue-soft/50"
                        )}
                      >
                        <td className="px-3 py-2.5">
                          <div className="font-mono-tech font-medium text-foreground">
                            {u.username}
                          </div>
                          <div className="text-[11px] text-muted-foreground">
                            {u.display_name || "—"}
                            {Object.keys(u.attributes ?? {}).length > 0 && (
                              <span className="ml-1.5 font-mono-tech text-xblue">
                                · {Object.keys(u.attributes).length} 项属性
                              </span>
                            )}
                          </div>
                        </td>
                        <td className="px-3 py-2.5 font-mono-tech">{u.dept_id}</td>
                        <td className="px-3 py-2.5">
                          <ClearanceBadge clearance={u.clearance} />
                        </td>
                        <td className="px-3 py-2.5">
                          <div className="flex flex-wrap gap-1">
                            {u.roles.length === 0 ? (
                              <span className="text-muted-foreground">—</span>
                            ) : (
                              u.roles.map((r) => (
                                <Badge key={r} variant="outline">
                                  {r}
                                </Badge>
                              ))
                            )}
                          </div>
                        </td>
                        <td className="px-3 py-2.5">
                          <div className="flex flex-wrap items-center gap-1">
                            <Badge variant="outline">{u.auth_source}</Badge>
                            <Badge
                              variant={u.status === "ACTIVE" ? "neutral" : "outline"}
                            >
                              {u.status === "ACTIVE" ? "正常" : "已停用"}
                            </Badge>
                            {locked && (
                              <Badge
                                variant="xblue"
                                className="gap-1"
                                title={`锁定至 ${u.locked_until ?? ""}`}
                              >
                                <Lock className="h-2.5 w-2.5" />
                                已锁定
                              </Badge>
                            )}
                          </div>
                        </td>
                        <td
                          className="px-3 py-2.5 text-right"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <div className="inline-flex items-center gap-1.5">
                            {(locked || u.failed_logins > 0) && (
                              <Button
                                variant="outline"
                                type="button"
                                onClick={() => void handleQuickUnlock(u)}
                                className="px-2 py-1 text-[11px]"
                                data-testid={`unlock-user-${u.username}`}
                              >
                                <Unlock className="h-3 w-3 text-xblue" />
                                解锁
                              </Button>
                            )}
                            <Button
                              variant="ghost"
                              type="button"
                              onClick={() => setSelectedUserId(u.id)}
                              className="px-2 py-1 text-[11px]"
                            >
                              <SlidersHorizontal className="h-3 w-3" />
                              详情
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

        {selectedUser && (
          <Card
            className="xl:col-span-5 p-4 space-y-4"
            data-testid="edit-user-drawer"
          >
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div>
                <div className="text-sm font-semibold flex items-center gap-2">
                  <span className="font-mono-tech">{selectedUser.username}</span>
                  <Badge variant="outline">{selectedUser.auth_source}</Badge>
                </div>
                <div className="text-[11px] font-mono-tech text-muted-foreground">
                  UUID: {selectedUser.id}
                </div>
              </div>
              <Button
                variant="ghost"
                type="button"
                onClick={() => setSelectedUserId(null)}
                className="p-1.5"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {/* Section 1: Basic Profile, Clearance, Roles & Status */}
            <div className="space-y-3">
              <div className="text-xs font-semibold">基础身份与密级授权</div>
              <div className="grid grid-cols-2 gap-2.5">
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">显示姓名</label>
                  <Input
                    value={editDisplayName}
                    onChange={(e) => setEditDisplayName(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">归属部门</label>
                  <select
                    value={editDeptId}
                    onChange={(e) => setEditDeptId(e.target.value)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {deptOptions.map((d) => (
                      <option key={d.dept_id} value={d.dept_id}>
                        {d.name} ({d.dept_id})
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    主体密级 (Clearance)
                  </label>
                  <select
                    value={editClearance}
                    onChange={(e) => setEditClearance(e.target.value as Sensitivity)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  >
                    {SENSITIVITY_OPTIONS.map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">账号状态</label>
                  <select
                    value={editStatus}
                    onChange={(e) =>
                      setEditStatus(e.target.value as UserAccountStatus)
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    <option value="ACTIVE">ACTIVE · 启用</option>
                    <option value="DISABLED">DISABLED · 停用</option>
                  </select>
                </div>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">绑定角色集合</label>
                <div className="flex flex-wrap gap-1.5 rounded-md border border-border p-2">
                  {roleNameOptions.map((roleName) => {
                    const checked = editRoles.includes(roleName);
                    return (
                      <label
                        key={roleName}
                        className={cn(
                          "inline-flex items-center gap-1.5 rounded px-2 py-1 text-[11px] font-mono-tech border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) => {
                            if (e.target.checked) {
                              setEditRoles((prev) => [...prev, roleName]);
                            } else {
                              setEditRoles((prev) =>
                                prev.filter((r) => r !== roleName)
                              );
                            }
                          }}
                          className="accent-[#2563eb]"
                        />
                        {roleName}
                      </label>
                    );
                  })}
                </div>
              </div>

              <div className="flex items-center justify-between gap-2">
                {isAccountLocked(selectedUser.locked_until) ? (
                  <Button
                    variant="outline"
                    type="button"
                    onClick={() => void handleQuickUnlock(selectedUser)}
                  >
                    <Unlock className="h-3.5 w-3.5 text-xblue" />
                    解除登录锁定
                  </Button>
                ) : (
                  <span className="text-[11px] text-muted-foreground">
                    连续失败次数: {selectedUser.failed_logins}
                  </span>
                )}
                <Button
                  type="button"
                  disabled={savingUser}
                  onClick={() => void handleSaveUser()}
                >
                  {savingUser ? "保存中..." : "保存身份配置"}
                </Button>
              </div>
            </div>

            {/* Section 2: User Attributes (`sec_user_attributes`) */}
            <div className="border-t border-border pt-3 space-y-2.5">
              <div className="flex items-center justify-between">
                <div className="text-xs font-semibold">
                  主体扩展属性（行策略占位符 user.attr.*）
                </div>
                <Button
                  variant="outline"
                  type="button"
                  disabled={savingAttrs}
                  onClick={() => void handleSaveAttributes()}
                  data-testid="save-user-attrs-btn"
                  className="px-2.5 py-1 text-[11px]"
                >
                  {savingAttrs ? "同步中..." : "保存属性"}
                </Button>
              </div>

              <div className="space-y-1.5">
                {Object.entries(attrDraft).length === 0 ? (
                  <div className="text-[11px] text-muted-foreground rounded border border-dashed border-border px-2.5 py-2">
                    暂无扩展属性（示例: warehouses = [&quot;W-1&quot;, &quot;W-2&quot;]）
                  </div>
                ) : (
                  Object.entries(attrDraft).map(([key, vals]) => (
                    <div
                      key={key}
                      className="flex items-start justify-between gap-2 rounded border border-border bg-muted/30 px-2.5 py-1.5 text-xs"
                    >
                      <div className="space-y-1 min-w-0">
                        <div className="font-mono-tech font-medium text-xblue">
                          {key}
                        </div>
                        <div className="flex flex-wrap gap-1">
                          {vals.map((val) => (
                            <Badge
                              key={val}
                              variant="outline"
                              className="gap-1 pr-1"
                            >
                              {val}
                              <button
                                type="button"
                                onClick={() =>
                                  setAttrDraft((prev) => ({
                                    ...prev,
                                    [key]: (prev[key] ?? []).filter((x) => x !== val),
                                  }))
                                }
                                className="hover:text-foreground cursor-pointer"
                              >
                                <X className="h-2.5 w-2.5" />
                              </button>
                            </Badge>
                          ))}
                        </div>
                      </div>
                      <button
                        type="button"
                        onClick={() =>
                          setAttrDraft((prev) => {
                            const next = { ...prev };
                            delete next[key];
                            return next;
                          })
                        }
                        className="text-muted-foreground hover:text-foreground p-1 cursor-pointer"
                        title="删除该属性键"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  ))
                )}
              </div>

              <div className="grid grid-cols-12 gap-1.5">
                <Input
                  placeholder="属性键 (如 warehouses)"
                  value={newAttrKey}
                  onChange={(e) => setNewAttrKey(e.target.value)}
                  className="col-span-5 h-8 text-xs font-mono-tech"
                />
                <Input
                  placeholder="逗号分隔值 (如 W-1, W-2)"
                  value={newAttrValuesCsv}
                  onChange={(e) => setNewAttrValuesCsv(e.target.value)}
                  className="col-span-5 h-8 text-xs font-mono-tech"
                />
                <Button
                  variant="outline"
                  type="button"
                  onClick={handleAddAttribute}
                  className="col-span-2 h-8 px-2"
                >
                  <Plus className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>

            {/* Section 3: Reset Local Password */}
            {selectedUser.auth_source === "LOCAL" && (
              <div className="border-t border-border pt-3 space-y-2">
                <div className="text-xs font-semibold flex items-center gap-1.5">
                  <KeyRound className="h-3.5 w-3.5 text-xblue" />
                  重置本地口令
                </div>
                <div className="flex gap-2">
                  <Input
                    type="password"
                    placeholder="新口令（≥ 12 位且不含用户名）"
                    value={resetPasswordVal}
                    onChange={(e) => setResetPasswordVal(e.target.value)}
                    className="h-8 text-xs"
                  />
                  <Button
                    variant="outline"
                    type="button"
                    disabled={resettingPassword || !resetPasswordVal}
                    onClick={() => void handleResetPassword()}
                    className="shrink-0 h-8"
                  >
                    重置
                  </Button>
                </div>
              </div>
            )}

            {/* Section 4: Active Refresh Sessions */}
            <div className="border-t border-border pt-3 space-y-2">
              <div className="flex items-center justify-between">
                <div className="text-xs font-semibold flex items-center gap-1.5">
                  <MonitorSmartphone className="h-3.5 w-3.5 text-xblue" />
                  刷新会话族（sec_sessions）
                </div>
                <button
                  type="button"
                  onClick={() => void loadUserSessions(selectedUser.id)}
                  className="text-[11px] text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  刷新列表
                </button>
              </div>
              {loadingSessions ? (
                <div className="text-[11px] text-muted-foreground">正在加载会话...</div>
              ) : sessions.length === 0 ? (
                <div className="text-[11px] text-muted-foreground">当前无活跃刷新会话</div>
              ) : (
                <div className="space-y-1.5 max-h-44 overflow-y-auto">
                  {sessions.map((s) => {
                    const active = !s.revoked_at && !s.rotated_at;
                    return (
                      <div
                        key={s.id}
                        className="flex items-center justify-between gap-2 rounded border border-border px-2.5 py-1.5 text-[11px]"
                      >
                        <div className="min-w-0">
                          <div className="font-mono-tech truncate">
                            Family {s.family_id.slice(0, 8)} · {s.client_ip || "local"} ·{" "}
                            {s.auth_source}
                          </div>
                          <div className="text-muted-foreground truncate">
                            {s.revoked_at
                              ? "已吊销"
                              : s.rotated_at
                                ? "已轮换"
                                : `活跃 · 到期 ${s.expires_at.slice(0, 10)}`}
                          </div>
                        </div>
                        {active && (
                          <Button
                            variant="outline"
                            type="button"
                            onClick={() => void handleRevokeSession(s.family_id)}
                            className="px-2 py-1 text-[11px] shrink-0"
                          >
                            吊销
                          </Button>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </Card>
        )}
      </div>

      {/* Create Local User Modal */}
      {createOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
          <Card className="w-full max-w-lg p-5 space-y-4">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="text-sm font-semibold flex items-center gap-2">
                <UserPlus className="h-4 w-4 text-xblue" />
                新建本地用户（Argon2id）
              </div>
              <button
                type="button"
                onClick={() => setCreateOpen(false)}
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <form onSubmit={(e) => void handleCreateUser(e)} className="space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-xs font-medium">用户名 *</label>
                  <Input
                    required
                    placeholder="如 analyst_east"
                    value={newUsername}
                    onChange={(e) => setNewUsername(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-xs font-medium">显示姓名</label>
                  <Input
                    placeholder="如 华东分析师"
                    value={newDisplayName}
                    onChange={(e) => setNewDisplayName(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="text-xs font-medium">归属部门</label>
                  <select
                    value={newDeptId}
                    onChange={(e) => setNewDeptId(e.target.value)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {deptOptions.map((d) => (
                      <option key={d.dept_id} value={d.dept_id}>
                        {d.name} ({d.dept_id})
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <label className="text-xs font-medium">安全密级</label>
                  <select
                    value={newClearance}
                    onChange={(e) => setNewClearance(e.target.value as Sensitivity)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  >
                    {SENSITIVITY_OPTIONS.map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium">电子邮箱</label>
                <Input
                  type="email"
                  placeholder="user@enterprise.internal"
                  value={newEmail}
                  onChange={(e) => setNewEmail(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium">初始登录口令 *</label>
                <Input
                  required
                  type="password"
                  placeholder="长度不少于 12 位，且不得包含用户名"
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  className="h-8 text-xs"
                />
                <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                  <ShieldAlert className="h-3 w-3 text-xblue shrink-0" />
                  <span>
                    口令策略：长度 ≥ 12 位且禁止包含用户名；以 Argon2id 哈希存储。
                  </span>
                </div>
                {createPwdHint && (
                  <div className="text-[11px] font-medium text-foreground">
                    提示：{createPwdHint}
                  </div>
                )}
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium">初始角色</label>
                <div className="flex flex-wrap gap-1.5">
                  {roleNameOptions.map((r) => {
                    const checked = newRoles.includes(r);
                    return (
                      <label
                        key={r}
                        className={cn(
                          "inline-flex items-center gap-1.5 rounded px-2 py-1 text-[11px] font-mono-tech border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) => {
                            if (e.target.checked) {
                              setNewRoles((prev) => [...prev, r]);
                            } else {
                              setNewRoles((prev) => prev.filter((x) => x !== r));
                            }
                          }}
                          className="accent-[#2563eb]"
                        />
                        {r}
                      </label>
                    );
                  })}
                </div>
              </div>

              <div className="flex justify-end gap-2 pt-2 border-t border-border">
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => setCreateOpen(false)}
                >
                  取消
                </Button>
                <Button
                  type="submit"
                  disabled={
                    creating ||
                    !newUsername.trim() ||
                    Boolean(validateLocalPassword(newPassword, newUsername))
                  }
                >
                  {creating ? "创建中..." : "确认创建用户"}
                </Button>
              </div>
            </form>
          </Card>
        </div>
      )}
    </div>
  );
}
