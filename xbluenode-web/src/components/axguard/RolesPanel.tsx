import { type FormEvent, useEffect, useMemo, useState } from "react";
import { Lock, Plus, Save, Shield, Trash2 } from "lucide-react";
import { ApiError } from "@/lib/api";
import { axguardApi, type GuardRoleRecord } from "@/lib/api/axguard";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { PERMISSION_DOMAIN_GROUPS } from "./constants";

interface RolesPanelProps {
  roles: GuardRoleRecord[];
  onReload: () => Promise<void>;
}

export function RolesPanel({ roles, onReload }: RolesPanelProps) {
  const [selectedRoleId, setSelectedRoleId] = useState<string | null>(null);
  const [draftPermissions, setDraftPermissions] = useState<string[]>([]);
  const [draftName, setDraftName] = useState("");
  const [newRoleName, setNewRoleName] = useState("");
  const [saving, setSaving] = useState(false);
  const [creating, setCreating] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);

  useEffect(() => {
    if (!selectedRoleId && roles.length > 0) {
      const first = roles[0];
      if (first) {
        setSelectedRoleId(first.id);
      }
    }
  }, [roles, selectedRoleId]);

  const selectedRole = useMemo(
    () => roles.find((r) => r.id === selectedRoleId) ?? null,
    [roles, selectedRoleId]
  );

  useEffect(() => {
    if (selectedRole) {
      setDraftPermissions([...selectedRole.permissions]);
      setDraftName(selectedRole.name);
    }
  }, [selectedRole]);

  const togglePermission = (code: string) => {
    setDraftPermissions((prev) =>
      prev.includes(code) ? prev.filter((p) => p !== code) : [...prev, code]
    );
  };

  const handleCreateRole = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const cleanName = newRoleName.trim().toUpperCase();
    if (!cleanName) return;
    setCreating(true);
    setBanner(null);
    try {
      const created = await axguardApi.createRole(cleanName, ["DATA_READ"]);
      setNewRoleName("");
      await onReload();
      setSelectedRoleId(created.id);
      setBanner(`已创建自定义角色 ${created.name}`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setCreating(false);
    }
  };

  const handleSaveRole = async () => {
    if (!selectedRole) return;
    setSaving(true);
    setBanner(null);
    try {
      await axguardApi.patchRole(selectedRole.id, {
        name: selectedRole.is_builtin ? undefined : draftName.trim() || undefined,
        permissions: draftPermissions,
      });
      await onReload();
      setBanner(`已更新角色 ${selectedRole.name} 的权限矩阵`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteRole = async () => {
    if (!selectedRole || selectedRole.is_builtin) return;
    setBanner(null);
    try {
      await axguardApi.deleteRole(selectedRole.id);
      setSelectedRoleId(null);
      await onReload();
      setBanner(`已删除自定义角色 ${selectedRole.name}`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
  };

  return (
    <div className="space-y-4" data-testid="axguard-roles-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        {/* Left Role List + Create Custom Role */}
        <Card className="lg:col-span-4 p-4 space-y-4">
          <div className="space-y-1">
            <div className="text-xs font-semibold flex items-center gap-1.5">
              <Shield className="h-3.5 w-3.5 text-xblue" />
              角色目录（内置与自定义）
            </div>
            <p className="text-[11px] text-muted-foreground">
              内置角色（ADMIN / ANALYST / OPERATOR / VIEWER）名称受平台保护不可删除。
            </p>
          </div>

          <div className="space-y-1.5">
            {roles.map((r) => {
              const active = r.id === selectedRoleId;
              return (
                <button
                  key={r.id}
                  type="button"
                  onClick={() => setSelectedRoleId(r.id)}
                  className={cn(
                    "w-full flex items-center justify-between rounded-md border px-3 py-2 text-left text-xs transition-colors cursor-pointer",
                    active
                      ? "border-xblue bg-xblue-soft/60 text-foreground"
                      : "border-border hover:bg-muted/50 text-foreground"
                  )}
                >
                  <div>
                    <div className="font-mono-tech font-semibold">{r.name}</div>
                    <div className="text-[11px] text-muted-foreground">
                      已授予 {r.permissions.length} 项原子权限
                    </div>
                  </div>
                  {r.is_builtin ? (
                    <Badge variant="outline" className="gap-1">
                      <Lock className="h-2.5 w-2.5" />
                      内置
                    </Badge>
                  ) : (
                    <Badge variant="xblue">自定义</Badge>
                  )}
                </button>
              );
            })}
          </div>

          <form
            onSubmit={(e) => void handleCreateRole(e)}
            className="border-t border-border pt-3 space-y-2"
          >
            <label className="block text-[11px] font-medium text-muted-foreground">
              新建自定义角色
            </label>
            <div className="flex gap-2">
              <Input
                placeholder="如 DATA_STEWARD"
                value={newRoleName}
                onChange={(e) => setNewRoleName(e.target.value)}
                data-testid="new-role-name-input"
                className="h-8 text-xs font-mono-tech"
              />
              <Button
                type="submit"
                disabled={creating || !newRoleName.trim()}
                className="h-8 shrink-0"
              >
                <Plus className="h-3.5 w-3.5" />
                新建
              </Button>
            </div>
          </form>
        </Card>

        {/* Right Interactive Permission Matrix */}
        <Card className="lg:col-span-8 p-4 space-y-4">
          {selectedRole ? (
            <>
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
                <div className="flex items-center gap-2">
                  {selectedRole.is_builtin ? (
                    <span className="font-mono-tech text-sm font-semibold">
                      {selectedRole.name}
                    </span>
                  ) : (
                    <Input
                      value={draftName}
                      onChange={(e) => setDraftName(e.target.value)}
                      className="h-8 w-48 font-mono-tech text-xs font-semibold"
                    />
                  )}
                  <Badge variant={selectedRole.is_builtin ? "outline" : "xblue"}>
                    {selectedRole.is_builtin ? "平台内置角色" : "租户自定义角色"}
                  </Badge>
                </div>

                <div className="flex items-center gap-2">
                  {!selectedRole.is_builtin && (
                    <Button
                      variant="outline"
                      type="button"
                      onClick={() => void handleDeleteRole()}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      删除角色
                    </Button>
                  )}
                  <Button
                    type="button"
                    disabled={saving}
                    onClick={() => void handleSaveRole()}
                    data-testid="save-role-permissions-btn"
                  >
                    <Save className="h-3.5 w-3.5" />
                    {saving ? "保存中..." : "保存权限矩阵"}
                  </Button>
                </div>
              </div>

              <div className="space-y-4">
                {PERMISSION_DOMAIN_GROUPS.map((group) => (
                  <div
                    key={group.domain}
                    className="rounded-md border border-border p-3 space-y-2.5"
                  >
                    <div>
                      <div className="text-xs font-semibold">{group.domain}</div>
                      <div className="text-[11px] text-muted-foreground">
                        {group.description}
                      </div>
                    </div>

                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                      {group.permissions.map((perm) => {
                        const checked = draftPermissions.includes(perm.code);
                        return (
                          <label
                            key={perm.code}
                            className={cn(
                              "flex items-center gap-2 rounded border px-2.5 py-2 text-xs transition-colors cursor-pointer",
                              checked
                                ? "border-xblue/60 bg-xblue-soft/40 text-foreground"
                                : "border-border hover:bg-muted/40 text-muted-foreground"
                            )}
                          >
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={() => togglePermission(perm.code)}
                              className="accent-[#2563eb]"
                            />
                            <span className="truncate">{perm.label}</span>
                          </label>
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>
            </>
          ) : (
            <div className="py-12 text-center text-xs text-muted-foreground">
              请从左侧选择一个角色以配置其原子权限矩阵
            </div>
          )}
        </Card>
      </div>
    </div>
  );
}
