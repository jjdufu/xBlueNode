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
import {
  Badge,
  Button,
  Card,
  Drawer,
  Input,
  Modal,
  PasswordInput,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { SENSITIVITY_OPTIONS } from "./constants";
import { isAccountLocked, validateLocalPassword } from "./logic";

interface UsersPanelProps {
  users: GuardUserRecord[];
  roles: GuardRoleRecord[];
  departments: DepartmentRecord[];
  onReload: () => Promise<void>;
}

const ATTR_KEY_PATTERN = /^[a-z][a-z0-9_]{0,62}$/;

function ClearanceBadge({ clearance }: { clearance: Sensitivity }) {
  const { tEnum } = useI18n();
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
      {tEnum(clearance)}
    </Badge>
  );
}

export function UsersPanel({
  users,
  roles,
  departments,
  onReload,
}: UsersPanelProps) {
  const { isZh, tx, tEnum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canWrite = can("GUARD_ADMIN");

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
      return [
        {
          tenant_id: "default",
          dept_id: "default",
          name: tx("默认部门", "Default"),
          parent_id: null,
        },
      ];
    }
    return departments;
  }, [departments, tx]);

  const handleQuickUnlock = async (user: GuardUserRecord) => {
    setBanner(null);
    try {
      await axguardApi.patchUser(user.id, { unlock: true });
      setBanner({
        kind: "ok",
        text: tx(
          `已解除账号 ${user.username} 的登录锁定`,
          `Unlocked account ${user.username}`
        ),
      });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
      setBanner({
        kind: "ok",
        text: tx(
          `本地账号 ${created.username} 已创建`,
          `Local account ${created.username} created`
        ),
      });
      await onReload();
      setSelectedUserId(created.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
      setBanner({
        kind: "ok",
        text: tx(
          `账号 ${selectedUser.username} 配置已保存`,
          `Account ${selectedUser.username} saved`
        ),
      });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
        text: tx(
          `已重置 ${selectedUser.username} 口令并吊销历史会话`,
          `Password reset and sessions revoked for ${selectedUser.username}`
        ),
      });
      await loadUserSessions(selectedUser.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
        text: tx(
          "属性键须以小写字母开头，仅含小写字母、数字或下划线",
          "Attribute key must start with a lowercase letter and contain only a-z, 0-9, or _"
        ),
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
        text: tx(
          `已保存 ${selectedUser.username} 扩展属性`,
          `Updated attributes for ${selectedUser.username}`
        ),
      });
      await onReload();
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
      setBanner({
        kind: "ok",
        text: tx(
          `已吊销会话族 ${familyId.slice(0, 8)}`,
          `Revoked session family ${familyId.slice(0, 8)}`
        ),
      });
      await loadUserSessions(selectedUser.id);
    } catch (err) {
      setBanner({
        kind: "err",
        text:
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
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
              placeholder={tx(
                "用户名 / 姓名 / 邮箱 / 角色",
                "Username / name / email / role"
              )}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              data-testid="users-search-input"
              className="pl-8 h-8 text-xs"
            />
          </div>
          <Select
            value={deptFilter}
            onChange={(e) => setDeptFilter(e.target.value)}
            className="h-8 text-xs"
            aria-label={tx("部门筛选", "Department filter")}
          >
            <option value="">{tx("全部部门", "All Departments")}</option>
            {deptOptions.map((d) => (
              <option key={d.dept_id} value={d.dept_id}>
                {d.name} ({d.dept_id})
              </option>
            ))}
          </Select>
        </div>

        <div className="flex items-center gap-2">
          <Button variant="outline" type="button" onClick={() => void onReload()}>
            <RefreshCw className="h-3.5 w-3.5" />
            {tx("刷新", "Refresh")}
          </Button>
          {canWrite && (
            <Button
              type="button"
              data-testid="open-create-user-btn"
              onClick={() => setCreateOpen(true)}
            >
              <UserPlus className="h-3.5 w-3.5" />
              {tx("新建用户", "New User")}
            </Button>
          )}
        </div>
      </div>

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("账号 / 姓名", "User / Name")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("部门", "Department")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("密级", "Clearance")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("角色", "Roles")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("来源 / 状态", "Source / Status")}
                </th>
                <th className="px-3 py-2.5 font-medium text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {filteredUsers.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    {tx("无匹配用户记录", "No matching users")}
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
                              ·{" "}
                              {tx(
                                `${Object.keys(u.attributes).length} 项属性`,
                                `${Object.keys(u.attributes).length} attrs`
                              )}
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
                              <Badge key={r} variant="outline" title={r}>
                                {tEnum(r)}
                              </Badge>
                            ))
                          )}
                        </div>
                      </td>
                      <td className="px-3 py-2.5">
                        <div className="flex flex-wrap items-center gap-1">
                          <Badge variant="outline">{tEnum(u.auth_source)}</Badge>
                          <StatusBadge status={u.status} />
                          {locked && (
                            <Badge
                              variant="danger"
                              className="gap-1"
                              title={fmtTime(u.locked_until)}
                            >
                              <Lock className="h-2.5 w-2.5" />
                              {tx("已锁定", "Locked")}
                            </Badge>
                          )}
                        </div>
                      </td>
                      <td
                        className="px-3 py-2.5 text-right"
                        onClick={(e) => e.stopPropagation()}
                      >
                        <div className="inline-flex items-center gap-1.5">
                          {canWrite && (locked || u.failed_logins > 0) && (
                            <Button
                              variant="outline"
                              size="sm"
                              type="button"
                              onClick={() => void handleQuickUnlock(u)}
                              data-testid={`unlock-user-${u.username}`}
                            >
                              <Unlock className="h-3 w-3 text-xblue" />
                              {tx("解锁", "Unlock")}
                            </Button>
                          )}
                          <Button
                            variant="ghost"
                            size="sm"
                            type="button"
                            onClick={() => setSelectedUserId(u.id)}
                          >
                            <SlidersHorizontal className="h-3 w-3" />
                            {tx("详情", "Details")}
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

      {/* Edit User Drawer */}
      <Drawer
        open={Boolean(selectedUser)}
        onClose={() => setSelectedUserId(null)}
        testId="edit-user-drawer"
        widthClass="max-w-xl"
        title={
          selectedUser ? (
            <div className="flex items-center gap-2 min-w-0">
              <span className="font-mono-tech truncate">
                {selectedUser.username}
              </span>
              <Badge variant="outline">{tEnum(selectedUser.auth_source)}</Badge>
            </div>
          ) : (
            tx("用户详情", "User Details")
          )
        }
      >
        {selectedUser && (
          <div className="space-y-4">
            <div className="text-[11px] font-mono-tech text-muted-foreground">
              ID: {selectedUser.id}
            </div>

            {/* Section 1: Basic Profile, Clearance, Roles & Status */}
            <div className="space-y-3">
              <div className="text-xs font-semibold">
                {tx("身份与密级", "Profile & Clearance")}
              </div>
              <div className="grid grid-cols-2 gap-2.5">
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("姓名", "Display Name")}
                  </label>
                  <Input
                    value={editDisplayName}
                    disabled={!canWrite}
                    onChange={(e) => setEditDisplayName(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("部门", "Department")}
                  </label>
                  <Select
                    value={editDeptId}
                    disabled={!canWrite}
                    onChange={(e) => setEditDeptId(e.target.value)}
                    className="h-8 w-full text-xs"
                  >
                    {deptOptions.map((d) => (
                      <option key={d.dept_id} value={d.dept_id}>
                        {d.name} ({d.dept_id})
                      </option>
                    ))}
                  </Select>
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("密级", "Clearance")}
                  </label>
                  <Select
                    value={editClearance}
                    disabled={!canWrite}
                    onChange={(e) =>
                      setEditClearance(e.target.value as Sensitivity)
                    }
                    className="h-8 w-full text-xs"
                  >
                    {SENSITIVITY_OPTIONS.map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {isZh ? opt.label : opt.enLabel}
                      </option>
                    ))}
                  </Select>
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("状态", "Status")}
                  </label>
                  <Select
                    value={editStatus}
                    disabled={!canWrite}
                    onChange={(e) =>
                      setEditStatus(e.target.value as UserAccountStatus)
                    }
                    className="h-8 w-full text-xs"
                  >
                    <option value="ACTIVE">{tEnum("ACTIVE")}</option>
                    <option value="DISABLED">{tEnum("DISABLED")}</option>
                  </Select>
                </div>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("绑定角色", "Assigned Roles")}
                </label>
                <div className="flex flex-wrap gap-1.5 rounded-md border border-border p-2">
                  {roleNameOptions.map((roleName) => {
                    const checked = editRoles.includes(roleName);
                    return (
                      <label
                        key={roleName}
                        title={roleName}
                        className={cn(
                          "inline-flex items-center gap-1.5 rounded px-2 py-1 text-[11px] border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={!canWrite}
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
                        <span>{tEnum(roleName)}</span>
                      </label>
                    );
                  })}
                </div>
              </div>

              {canWrite && (
                <div className="flex items-center justify-between gap-2">
                  {isAccountLocked(selectedUser.locked_until) ? (
                    <Button
                      variant="outline"
                      size="sm"
                      type="button"
                      onClick={() => void handleQuickUnlock(selectedUser)}
                    >
                      <Unlock className="h-3.5 w-3.5 text-xblue" />
                      {tx("解除锁定", "Unlock")}
                    </Button>
                  ) : (
                    <span className="text-[11px] text-muted-foreground">
                      {tx(
                        `连续失败: ${selectedUser.failed_logins}`,
                        `Failed logins: ${selectedUser.failed_logins}`
                      )}
                    </span>
                  )}
                  <Button
                    size="sm"
                    type="button"
                    disabled={savingUser}
                    onClick={() => void handleSaveUser()}
                  >
                    {savingUser
                      ? tx("保存中…", "Saving…")
                      : tx("保存配置", "Save Profile")}
                  </Button>
                </div>
              )}
            </div>

            {/* Section 2: User Attributes (`sec_user_attributes`) */}
            <div className="border-t border-border pt-3 space-y-2.5">
              <div className="flex items-center justify-between">
                <div className="text-xs font-semibold">
                  {tx("扩展属性", "Subject Attributes")}
                </div>
                {canWrite && (
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    disabled={savingAttrs}
                    onClick={() => void handleSaveAttributes()}
                    data-testid="save-user-attrs-btn"
                  >
                    {savingAttrs
                      ? tx("同步中…", "Saving…")
                      : tx("保存属性", "Save Attributes")}
                  </Button>
                )}
              </div>

              <div className="space-y-1.5">
                {Object.entries(attrDraft).length === 0 ? (
                  <div className="text-[11px] text-muted-foreground rounded border border-dashed border-border px-2.5 py-2 font-mono-tech">
                    {tx(
                      '暂无扩展属性 · warehouses = ["W-1", "W-2"]',
                      'No attributes · warehouses = ["W-1", "W-2"]'
                    )}
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
                              {canWrite && (
                                <button
                                  type="button"
                                  onClick={() =>
                                    setAttrDraft((prev) => ({
                                      ...prev,
                                      [key]: (prev[key] ?? []).filter(
                                        (x) => x !== val
                                      ),
                                    }))
                                  }
                                  className="hover:text-foreground cursor-pointer"
                                >
                                  <X className="h-2.5 w-2.5" />
                                </button>
                              )}
                            </Badge>
                          ))}
                        </div>
                      </div>
                      {canWrite && (
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
                          title={tx("删除属性", "Remove attribute")}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  ))
                )}
              </div>

              {canWrite && (
                <div className="grid grid-cols-12 gap-1.5">
                  <Input
                    placeholder="warehouses"
                    value={newAttrKey}
                    onChange={(e) => setNewAttrKey(e.target.value)}
                    className="col-span-5 h-8 text-xs font-mono-tech"
                  />
                  <Input
                    placeholder="W-1, W-2"
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
              )}
            </div>

            {/* Section 3: Reset Local Password */}
            {canWrite && selectedUser.auth_source === "LOCAL" && (
              <div className="border-t border-border pt-3 space-y-2">
                <div className="text-xs font-semibold flex items-center gap-1.5">
                  <KeyRound className="h-3.5 w-3.5 text-xblue" />
                  {tx("重置本地口令", "Reset Local Password")}
                </div>
                <div className="flex gap-2">
                  <div className="flex-1">
                    <PasswordInput
                      placeholder={tx("≥ 12 位新口令", "Min 12 characters")}
                      value={resetPasswordVal}
                      onChange={(e) => setResetPasswordVal(e.target.value)}
                      className="h-8 text-xs"
                    />
                  </div>
                  <Button
                    variant="outline"
                    type="button"
                    disabled={resettingPassword || !resetPasswordVal}
                    onClick={() => void handleResetPassword()}
                    className="shrink-0 h-8"
                  >
                    {tx("重置", "Reset")}
                  </Button>
                </div>
              </div>
            )}

            {/* Section 4: Active Refresh Sessions */}
            <div className="border-t border-border pt-3 space-y-2">
              <div className="flex items-center justify-between">
                <div className="text-xs font-semibold flex items-center gap-1.5">
                  <MonitorSmartphone className="h-3.5 w-3.5 text-xblue" />
                  {tx("活跃会话", "Refresh Sessions")}
                </div>
                <button
                  type="button"
                  onClick={() => void loadUserSessions(selectedUser.id)}
                  className="text-[11px] text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  {tx("刷新", "Refresh")}
                </button>
              </div>
              {loadingSessions ? (
                <div className="text-[11px] text-muted-foreground">
                  {tx("加载中…", "Loading…")}
                </div>
              ) : sessions.length === 0 ? (
                <div className="text-[11px] text-muted-foreground">
                  {tx("无活跃会话", "No active sessions")}
                </div>
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
                            {s.family_id.slice(0, 8)} · {s.client_ip || "local"}{" "}
                            · {tEnum(s.auth_source)}
                          </div>
                          <div className="text-muted-foreground truncate">
                            {s.revoked_at
                              ? tx("已吊销", "Revoked")
                              : s.rotated_at
                                ? tx("已轮换", "Rotated")
                                : `${tx("有效至", "Expires")} ${fmtTime(s.expires_at, true)}`}
                          </div>
                        </div>
                        {canWrite && active && (
                          <Button
                            variant="outline"
                            size="sm"
                            type="button"
                            onClick={() =>
                              void handleRevokeSession(s.family_id)
                            }
                          >
                            {tx("吊销", "Revoke")}
                          </Button>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
        )}
      </Drawer>

      {/* Create Local User Modal */}
      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        testId="create-user-modal"
        title={
          <>
            <UserPlus className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("新建本地用户", "New Local User")}</span>
          </>
        }
      >
        <form onSubmit={(e) => void handleCreateUser(e)} className="space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("用户名", "Username")} *
              </label>
              <Input
                required
                placeholder="analyst_east"
                value={newUsername}
                onChange={(e) => setNewUsername(e.target.value)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("姓名", "Display Name")}
              </label>
              <Input
                placeholder={tx("华东分析师", "East Analyst")}
                value={newDisplayName}
                onChange={(e) => setNewDisplayName(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("部门", "Department")}
              </label>
              <Select
                value={newDeptId}
                onChange={(e) => setNewDeptId(e.target.value)}
                className="h-8 w-full text-xs"
              >
                {deptOptions.map((d) => (
                  <option key={d.dept_id} value={d.dept_id}>
                    {d.name} ({d.dept_id})
                  </option>
                ))}
              </Select>
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium">
                {tx("密级", "Clearance")}
              </label>
              <Select
                value={newClearance}
                onChange={(e) => setNewClearance(e.target.value as Sensitivity)}
                className="h-8 w-full text-xs"
              >
                {SENSITIVITY_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {isZh ? opt.label : opt.enLabel}
                  </option>
                ))}
              </Select>
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("邮箱", "Email")}
            </label>
            <Input
              type="email"
              placeholder="user@enterprise.internal"
              value={newEmail}
              onChange={(e) => setNewEmail(e.target.value)}
              className="h-8 text-xs"
            />
          </div>

          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("初始口令", "Initial Password")} *
            </label>
            <PasswordInput
              required
              placeholder="Ops#Admin2026!Safe"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              className="h-8 text-xs"
            />
            <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <ShieldAlert className="h-3 w-3 text-xblue shrink-0" />
              <span>
                {tx(
                  "长度 ≥ 12 位且不含用户名，Argon2id 哈希存储。",
                  "Minimum 12 characters without username; stored via Argon2id."
                )}
              </span>
            </div>
            {createPwdHint && (
              <div className="text-[11px] font-medium text-amber-600 dark:text-amber-400">
                {createPwdHint}
              </div>
            )}
          </div>

          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("初始角色", "Initial Roles")}
            </label>
            <div className="flex flex-wrap gap-1.5">
              {roleNameOptions.map((r) => {
                const checked = newRoles.includes(r);
                return (
                  <label
                    key={r}
                    title={r}
                    className={cn(
                      "inline-flex items-center gap-1.5 rounded px-2 py-1 text-[11px] border cursor-pointer",
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
                    <span>{tEnum(r)}</span>
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
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="submit"
              disabled={
                creating ||
                !newUsername.trim() ||
                Boolean(validateLocalPassword(newPassword, newUsername))
              }
            >
              {creating ? tx("创建中…", "Creating…") : tx("创建", "Create")}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
