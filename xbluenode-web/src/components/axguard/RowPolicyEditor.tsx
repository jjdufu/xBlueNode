import { useEffect, useMemo, useState } from "react";
import {
  Eye,
  Filter,
  Plus,
  Save,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import { ApiError } from "@/lib/api";
import type { Dataset } from "@/lib/api/axhub";
import type { BlueNodeTypeDef } from "@/lib/api/xblue";
import {
  axguardApi,
  type DepartmentRecord,
  type GuardRoleRecord,
  type GuardUserRecord,
  type PolicyTargetKind,
  type RowPolicyDefaultRecord,
  type RowPolicyPreviewResponse,
  type RowPolicyRecord,
} from "@/lib/api/axguard";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { ROW_POLICY_PLACEHOLDER_CHIPS } from "./constants";
import { formatSubjectSelector } from "./logic";

interface RowPolicyEditorProps {
  rowPolicies: RowPolicyRecord[];
  rowDefaults: RowPolicyDefaultRecord[];
  datasets: Dataset[];
  nodeTypes: BlueNodeTypeDef[];
  users: GuardUserRecord[];
  roles: GuardRoleRecord[];
  departments: DepartmentRecord[];
  onReload: () => Promise<void>;
}

export function RowPolicyEditor({
  rowPolicies,
  rowDefaults,
  datasets,
  nodeTypes,
  users,
  roles,
  departments,
  onReload,
}: RowPolicyEditorProps) {
  const [targetKind, setTargetKind] = useState<PolicyTargetKind>("DATASET");
  const [targetRef, setTargetRef] = useState("");

  // Editor form state
  const [editingId, setEditingId] = useState<string | null>(null);
  const [policyName, setPolicyName] = useState("east_warehouses");
  const [predicate, setPredicate] = useState(
    "warehouse_code IN {{user.attr.warehouses}}"
  );
  const [enabled, setEnabled] = useState(true);
  const [selectedRoles, setSelectedRoles] = useState<string[]>(["ANALYST"]);
  const [selectedDepts, setSelectedDepts] = useState<string[]>([]);
  const [selectedUsers, setSelectedUsers] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [togglingDefault, setTogglingDefault] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);

  // Preview 20 rows state
  const [previewUserId, setPreviewUserId] = useState("");
  const [previewDatasetId, setPreviewDatasetId] = useState("");
  const [previewing, setPreviewing] = useState(false);
  const [previewResult, setPreviewResult] =
    useState<RowPolicyPreviewResponse | null>(null);

  useEffect(() => {
    if (!targetRef) {
      if (targetKind === "DATASET" && datasets[0]) {
        setTargetRef(datasets[0].id);
      } else if (targetKind === "NODE_TYPE" && nodeTypes[0]) {
        setTargetRef(nodeTypes[0].name);
      }
    }
  }, [targetKind, targetRef, datasets, nodeTypes]);

  useEffect(() => {
    if (!previewUserId && users[0]) {
      setPreviewUserId(users[0].id);
    }
  }, [users, previewUserId]);

  useEffect(() => {
    if (!previewDatasetId && datasets[0]) {
      setPreviewDatasetId(datasets[0].id);
    }
  }, [datasets, previewDatasetId]);

  const currentDefaultDeny = useMemo(() => {
    const found = rowDefaults.find(
      (d) => d.target_kind === targetKind && d.target_ref === targetRef
    );
    return found?.default_deny ?? false;
  }, [rowDefaults, targetKind, targetRef]);

  const matchingPolicies = useMemo(() => {
    if (!targetRef.trim()) {
      return rowPolicies.filter((p) => p.target_kind === targetKind);
    }
    return rowPolicies.filter(
      (p) => p.target_kind === targetKind && p.target_ref === targetRef
    );
  }, [rowPolicies, targetKind, targetRef]);

  const roleNames = useMemo(() => {
    const set = new Set<string>(["ANALYST", "OPERATOR", "VIEWER"]);
    for (const r of roles) {
      if (r.name !== "ADMIN") {
        set.add(r.name);
      }
    }
    return Array.from(set);
  }, [roles]);

  const startNewPolicy = () => {
    setEditingId(null);
    setPolicyName("");
    setPredicate("");
    setEnabled(true);
    setSelectedRoles([]);
    setSelectedDepts([]);
    setSelectedUsers([]);
  };

  const startEditPolicy = (p: RowPolicyRecord) => {
    setEditingId(p.id);
    setTargetKind(p.target_kind);
    setTargetRef(p.target_ref);
    setPolicyName(p.name);
    setPredicate(p.predicate);
    setEnabled(p.enabled);
    setSelectedRoles([...(p.applies_to?.roles ?? [])]);
    setSelectedDepts([...(p.applies_to?.depts ?? [])]);
    setSelectedUsers([...(p.applies_to?.users ?? [])]);
  };

  const handleToggleDefaultDeny = async (nextVal: boolean) => {
    if (!targetRef.trim()) {
      setBanner("请先选择或输入目标数据集 / 节点类型标识");
      return;
    }
    setTogglingDefault(true);
    setBanner(null);
    try {
      await axguardApi.putRowPolicyDefault({
        target_kind: targetKind,
        target_ref: targetRef.trim(),
        default_deny: nextVal,
      });
      await onReload();
      setBanner(
        nextVal
          ? `已对 ${targetRef} 开启无策略命中默认拒绝 (default_deny = true)`
          : `已对 ${targetRef} 关闭默认拒绝 (default_deny = false)`
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setTogglingDefault(false);
    }
  };

  const handleSavePolicy = async () => {
    const cleanName = policyName.trim();
    const cleanTarget = targetRef.trim();
    const cleanPred = predicate.trim();
    if (!cleanName || !cleanTarget || !cleanPred) {
      setBanner("请完整填写策略名称、目标对象与行过滤表达式");
      return;
    }

    setSaving(true);
    setBanner(null);
    const payload = {
      name: cleanName,
      target_kind: targetKind,
      target_ref: cleanTarget,
      applies_to: {
        roles: selectedRoles,
        depts: selectedDepts,
        users: selectedUsers,
      },
      predicate: cleanPred,
      enabled,
    };

    try {
      if (editingId) {
        await axguardApi.updateRowPolicy(editingId, payload);
        setBanner(`已更新行级安全策略 ${cleanName}`);
      } else {
        await axguardApi.createRowPolicy(payload);
        setBanner(`已创建行级安全策略 ${cleanName}`);
      }
      await onReload();
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSaving(false);
    }
  };

  const handleDeletePolicy = async (id: string, name: string) => {
    setBanner(null);
    try {
      await axguardApi.deleteRowPolicy(id);
      if (editingId === id) {
        startNewPolicy();
      }
      await onReload();
      setBanner(`已删除行级安全策略 ${name}`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
  };

  const handlePreviewRows = async () => {
    const uid = previewUserId.trim();
    const dsId = previewDatasetId.trim();
    if (!uid || !dsId) {
      setBanner("请选择预览视角用户与目标数据集 UUID");
      return;
    }
    setPreviewing(true);
    setBanner(null);
    try {
      const res = await axguardApi.previewRowPolicy(uid, dsId);
      setPreviewResult(res);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setPreviewing(false);
    }
  };

  const appendChip = (token: string) => {
    setPredicate((prev) => (prev ? `${prev} ${token}` : token));
  };

  return (
    <div className="space-y-4" data-testid="axguard-row-policies-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      {/* Target Selector + default_deny Toggle */}
      <Card className="p-4 flex flex-wrap items-center justify-between gap-4">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center rounded-md border border-border p-0.5 bg-muted/40">
            <button
              type="button"
              onClick={() => {
                setTargetKind("DATASET");
                setTargetRef(datasets[0]?.id ?? "");
              }}
              className={cn(
                "rounded px-2.5 py-1 text-xs font-medium cursor-pointer",
                targetKind === "DATASET"
                  ? "bg-background text-foreground shadow-xs"
                  : "text-muted-foreground"
              )}
            >
              数据集 (DATASET)
            </button>
            <button
              type="button"
              onClick={() => {
                setTargetKind("NODE_TYPE");
                setTargetRef(nodeTypes[0]?.name ?? "");
              }}
              className={cn(
                "rounded px-2.5 py-1 text-xs font-medium cursor-pointer",
                targetKind === "NODE_TYPE"
                  ? "bg-background text-foreground shadow-xs"
                  : "text-muted-foreground"
              )}
            >
              节点类型 (NODE_TYPE)
            </button>
          </div>

          {targetKind === "DATASET" && datasets.length > 0 ? (
            <select
              value={targetRef}
              onChange={(e) => setTargetRef(e.target.value)}
              className="h-8 rounded-md border border-input bg-background px-2.5 text-xs"
            >
              {datasets.map((ds) => (
                <option key={ds.id} value={ds.id}>
                  {ds.display_name || ds.name} ({ds.name})
                </option>
              ))}
            </select>
          ) : targetKind === "NODE_TYPE" && nodeTypes.length > 0 ? (
            <select
              value={targetRef}
              onChange={(e) => setTargetRef(e.target.value)}
              className="h-8 rounded-md border border-input bg-background px-2.5 text-xs"
            >
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.display_name || nt.name} ({nt.name})
                </option>
              ))}
            </select>
          ) : null}

          <Input
            placeholder={
              targetKind === "DATASET"
                ? "数据集 UUID（如 0000...）"
                : "节点类型名（如 Warehouse）"
            }
            value={targetRef}
            onChange={(e) => setTargetRef(e.target.value)}
            data-testid="row-target-ref-input"
            className="h-8 w-64 text-xs font-mono-tech"
          />
        </div>

        <label className="flex items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-1.5 text-xs cursor-pointer">
          <input
            type="checkbox"
            checked={currentDefaultDeny}
            disabled={togglingDefault || !targetRef.trim()}
            onChange={(e) => void handleToggleDefaultDeny(e.target.checked)}
            data-testid="row-default-deny-toggle"
            className="accent-[#2563eb]"
          />
          <ShieldAlert className="h-3.5 w-3.5 text-xblue" />
          <span>无匹配策略默认拒绝（default_deny = 0 行）</span>
        </label>
      </Card>

      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
        {/* Left: Row Policy List */}
        <Card className="xl:col-span-5 p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div>
              <div className="text-xs font-semibold flex items-center gap-1.5">
                <Filter className="h-3.5 w-3.5 text-xblue" />
                行级安全策略列表（同目标多策略按 OR 合并）
              </div>
              <p className="text-[11px] text-muted-foreground">
                DATA_ADMIN 与 GUARD_ADMIN 默认豁免行级过滤。
              </p>
            </div>
            <Button variant="outline" type="button" onClick={startNewPolicy}>
              <Plus className="h-3.5 w-3.5" />
              新建
            </Button>
          </div>

          {matchingPolicies.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted-foreground">
              当前目标对象暂无行级过滤策略
            </div>
          ) : (
            <div className="space-y-2">
              {matchingPolicies.map((p) => (
                <div
                  key={p.id}
                  onClick={() => startEditPolicy(p)}
                  className={cn(
                    "rounded-md border p-3 text-xs space-y-1.5 cursor-pointer transition-colors",
                    editingId === p.id
                      ? "border-xblue bg-xblue-soft/50"
                      : "border-border hover:bg-muted/40"
                  )}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-mono-tech font-semibold">{p.name}</span>
                    <div
                      className="flex items-center gap-1.5"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Badge variant={p.enabled ? "xblue" : "outline"}>
                        {p.enabled ? "启用" : "停用"}
                      </Badge>
                      <button
                        type="button"
                        onClick={() => void handleDeletePolicy(p.id, p.name)}
                        className="text-muted-foreground hover:text-foreground p-1 cursor-pointer"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>
                  <div className="rounded bg-muted/60 px-2 py-1 font-mono-tech text-[11px] break-all">
                    {p.predicate}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    适用主体：{formatSubjectSelector(p.applies_to, "全部非豁免主体")}
                  </div>
                </div>
              ))}
            </div>
          )}
        </Card>

        {/* Right: Row Policy Expression Builder */}
        <Card className="xl:col-span-7 p-4 space-y-3">
          <div className="border-b border-border pb-2.5 flex items-center justify-between">
            <div className="text-xs font-semibold">
              {editingId ? `编辑行策略 (${policyName})` : "新建行策略与主体选择器"}
            </div>
            <label className="inline-flex items-center gap-1.5 text-xs cursor-pointer">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
                className="accent-[#2563eb]"
              />
              策略启用
            </label>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">策略名称 *</label>
            <Input
              placeholder="如 east_warehouses"
              value={policyName}
              onChange={(e) => setPolicyName(e.target.value)}
              data-testid="row-policy-name-input"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div className="space-y-1.5">
            <label className="text-[11px] text-muted-foreground">
              适用主体 (applies_to · 全不选表示匹配全部主体)
            </label>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
              <div className="rounded border border-border p-2 space-y-1">
                <div className="text-[11px] font-medium">按角色</div>
                <div className="flex flex-wrap gap-1">
                  {roleNames.map((r) => {
                    const checked = selectedRoles.includes(r);
                    return (
                      <label
                        key={r}
                        className={cn(
                          "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-mono-tech border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) =>
                            setSelectedRoles((prev) =>
                              e.target.checked
                                ? [...prev, r]
                                : prev.filter((x) => x !== r)
                            )
                          }
                          className="accent-[#2563eb]"
                        />
                        {r}
                      </label>
                    );
                  })}
                </div>
              </div>

              <div className="rounded border border-border p-2 space-y-1">
                <div className="text-[11px] font-medium">按部门</div>
                <div className="flex flex-wrap gap-1">
                  {departments.map((d) => {
                    const checked = selectedDepts.includes(d.dept_id);
                    return (
                      <label
                        key={d.dept_id}
                        className={cn(
                          "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-mono-tech border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={(e) =>
                            setSelectedDepts((prev) =>
                              e.target.checked
                                ? [...prev, d.dept_id]
                                : prev.filter((x) => x !== d.dept_id)
                            )
                          }
                          className="accent-[#2563eb]"
                        />
                        {d.dept_id}
                      </label>
                    );
                  })}
                </div>
              </div>
            </div>
          </div>

          <div className="space-y-1.5">
            <div className="flex flex-wrap items-center justify-between gap-1">
              <label className="text-[11px] text-muted-foreground">
                行过滤布尔表达式（白名单 AST 解析，禁止函数与子查询）
              </label>
              <div className="flex flex-wrap gap-1">
                {ROW_POLICY_PLACEHOLDER_CHIPS.map((chip) => (
                  <button
                    key={chip.token}
                    type="button"
                    title={chip.hint}
                    onClick={() => appendChip(chip.token)}
                    className="rounded border border-border bg-muted/50 hover:bg-muted px-1.5 py-0.5 text-[11px] font-mono-tech text-xblue cursor-pointer"
                  >
                    {chip.token}
                  </button>
                ))}
              </div>
            </div>

            <Textarea
              rows={3}
              placeholder="如 warehouse_code IN {{user.attr.warehouses}} AND status <> 'DRAFT'"
              value={predicate}
              onChange={(e) => setPredicate(e.target.value)}
              data-testid="row-policy-predicate-input"
              className="font-mono-tech text-xs"
            />
          </div>

          <div className="flex justify-end">
            <Button
              type="button"
              disabled={
                saving ||
                !policyName.trim() ||
                !targetRef.trim() ||
                !predicate.trim()
              }
              onClick={() => void handleSavePolicy()}
              data-testid="save-row-policy-btn"
            >
              <Save className="h-3.5 w-3.5" />
              {saving ? "保存中..." : editingId ? "更新行策略" : "创建行策略"}
            </Button>
          </div>
        </Card>
      </div>

      {/* Live 20-Row User Perspective Preview */}
      <Card className="p-4 space-y-3" data-testid="row-policy-preview-card">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
          <div>
            <div className="text-xs font-semibold flex items-center gap-1.5">
              <Eye className="h-3.5 w-3.5 text-xblue" />
              以指定用户视角预览前 20 行（含实时行过滤与列脱敏效果）
            </div>
            <p className="text-[11px] text-muted-foreground">
              调用 POST /api/axguard/row-policies/preview 在 sql_guard 安全子查询改写下检视目标用户的真实可见样本。
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {users.length > 0 ? (
              <select
                value={previewUserId}
                onChange={(e) => setPreviewUserId(e.target.value)}
                data-testid="row-preview-user-select"
                className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              >
                {users.map((u) => (
                  <option key={u.id} value={u.id}>
                    视角: {u.username} ({u.dept_id})
                  </option>
                ))}
              </select>
            ) : (
              <Input
                placeholder="用户 UUID"
                value={previewUserId}
                onChange={(e) => setPreviewUserId(e.target.value)}
                data-testid="row-preview-user-select"
                className="h-8 w-44 text-xs font-mono-tech"
              />
            )}

            {datasets.length > 0 ? (
              <select
                value={previewDatasetId}
                onChange={(e) => setPreviewDatasetId(e.target.value)}
                data-testid="row-preview-dataset-select"
                className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              >
                {datasets.map((ds) => (
                  <option key={ds.id} value={ds.id}>
                    数据集: {ds.display_name || ds.name}
                  </option>
                ))}
              </select>
            ) : (
              <Input
                placeholder="数据集 UUID"
                value={previewDatasetId}
                onChange={(e) => setPreviewDatasetId(e.target.value)}
                data-testid="row-preview-dataset-select"
                className="h-8 w-48 text-xs font-mono-tech"
              />
            )}

            <Button
              type="button"
              disabled={previewing}
              onClick={() => void handlePreviewRows()}
              data-testid="row-policy-preview-btn"
            >
              <Eye className="h-3.5 w-3.5" />
              {previewing ? "查询预览中..." : "预览前 20 行"}
            </Button>
          </div>
        </div>

        {previewResult && (
          <div className="space-y-2" data-testid="row-policy-preview-table">
            <div className="text-[11px] font-mono-tech text-muted-foreground">
              共返回 {previewResult.row_count} 行可见记录
            </div>
            <div className="overflow-x-auto rounded border border-border">
              <table className="w-full text-xs border-collapse">
                <thead>
                  <tr className="border-b border-border bg-muted/40 text-left">
                    {previewResult.columns.map((col) => (
                      <th
                        key={col.name}
                        className="px-3 py-2 font-mono-tech font-medium"
                      >
                        {col.name}
                        <span className="ml-1 text-[10px] text-muted-foreground">
                          ({col.data_type})
                        </span>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-border">
                  {previewResult.rows.length === 0 ? (
                    <tr>
                      <td
                        colSpan={Math.max(1, previewResult.columns.length)}
                        className="px-4 py-6 text-center text-muted-foreground"
                      >
                        该用户视角下可见行数为 0
                      </td>
                    </tr>
                  ) : (
                    previewResult.rows.map((row, rIdx) => (
                      <tr key={rIdx} className="hover:bg-muted/30">
                        {previewResult.columns.map((col) => (
                          <td
                            key={col.name}
                            className="px-3 py-1.5 font-mono-tech whitespace-nowrap"
                          >
                            {row[col.name] === null || row[col.name] === undefined
                              ? "NULL"
                              : String(row[col.name])}
                          </td>
                        ))}
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </div>
        )}
      </Card>
    </div>
  );
}
