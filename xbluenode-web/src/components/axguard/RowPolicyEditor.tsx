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
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Select,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
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
  const { tx, tEnum, fmtNum } = useI18n();
  const { canAny } = usePrincipal();
  const canWrite = canAny(["GUARD_ADMIN", "DATA_ADMIN"]);

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
      setBanner(tx("目标对象标识不能为空", "Target identifier is required"));
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
          ? tx(
              `已开启无匹配策略默认拒绝 (${targetRef})`,
              `Enabled default deny for ${targetRef}`
            )
          : tx(
              `已关闭无匹配策略默认拒绝 (${targetRef})`,
              `Disabled default deny for ${targetRef}`
            )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
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
      setBanner(
        tx(
          "策略名称、目标对象与过滤表达式不能为空",
          "Policy name, target, and predicate are required"
        )
      );
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
        setBanner(
          tx(`已保存行策略 ${cleanName}`, `Updated row policy ${cleanName}`)
        );
      } else {
        await axguardApi.createRowPolicy(payload);
        setBanner(
          tx(`已创建行策略 ${cleanName}`, `Created row policy ${cleanName}`)
        );
      }
      await onReload();
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
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
      setBanner(tx(`已删除行策略 ${name}`, `Deleted row policy ${name}`));
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
      );
    }
  };

  const handlePreviewRows = async () => {
    const uid = previewUserId.trim();
    const dsId = previewDatasetId.trim();
    if (!uid || !dsId) {
      setBanner(
        tx(
          "预览用户与数据集不能为空",
          "Preview user and dataset are required"
        )
      );
      return;
    }
    setPreviewing(true);
    setBanner(null);
    try {
      const res = await axguardApi.previewRowPolicy(uid, dsId);
      setPreviewResult(res);
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
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
              {tx("数据集", "Dataset")}
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
              {tx("节点类型", "Node Type")}
            </button>
          </div>

          {targetKind === "DATASET" && datasets.length > 0 ? (
            <Select
              value={targetRef}
              onChange={(e) => setTargetRef(e.target.value)}
              className="h-8 text-xs"
            >
              {datasets.map((ds) => (
                <option key={ds.id} value={ds.id}>
                  {ds.display_name || ds.name} ({ds.name})
                </option>
              ))}
            </Select>
          ) : targetKind === "NODE_TYPE" && nodeTypes.length > 0 ? (
            <Select
              value={targetRef}
              onChange={(e) => setTargetRef(e.target.value)}
              className="h-8 text-xs"
            >
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.display_name || nt.name} ({nt.name})
                </option>
              ))}
            </Select>
          ) : null}

          <Input
            placeholder={
              targetKind === "DATASET"
                ? "00000000-0000-0000-0000-000000001310"
                : "Warehouse"
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
            disabled={!canWrite || togglingDefault || !targetRef.trim()}
            onChange={(e) => void handleToggleDefaultDeny(e.target.checked)}
            data-testid="row-default-deny-toggle"
            className="accent-[#2563eb]"
          />
          <ShieldAlert className="h-3.5 w-3.5 text-xblue" />
          <span>{tx("无匹配策略默认拒绝", "Default Deny When Unmatched")}</span>
        </label>
      </Card>

      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
        {/* Left: Row Policy List */}
        <Card className="xl:col-span-5 p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div>
              <div className="text-xs font-semibold flex items-center gap-1.5">
                <Filter className="h-3.5 w-3.5 text-xblue" />
                {tx("行级策略列表", "Row Security Policies")}
              </div>
              <p className="text-[11px] text-muted-foreground">
                {tx(
                  "同目标多条策略以逻辑或合并；管理员默认豁免。",
                  "Multiple policies on one target combine via OR; admins are exempt."
                )}
              </p>
            </div>
            {canWrite && (
              <Button
                variant="outline"
                size="sm"
                type="button"
                onClick={startNewPolicy}
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建", "New")}
              </Button>
            )}
          </div>

          {matchingPolicies.length === 0 ? (
            <EmptyState
              title={tx("当前目标暂无行级策略", "No row policies for this target")}
            />
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
                      <Badge variant={p.enabled ? "success" : "outline"}>
                        {p.enabled
                          ? tx("启用", "Enabled")
                          : tx("停用", "Disabled")}
                      </Badge>
                      {canWrite && (
                        <button
                          type="button"
                          onClick={() => void handleDeletePolicy(p.id, p.name)}
                          className="text-muted-foreground hover:text-foreground p-1 cursor-pointer"
                          title={tx("删除", "Delete")}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  </div>
                  <div className="rounded bg-muted/60 px-2 py-1 font-mono-tech text-[11px] break-all">
                    {p.predicate}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {tx("适用主体：", "Subjects: ")}
                    {formatSubjectSelector(
                      p.applies_to,
                      tx("全部非豁免主体", "All non-exempt principals")
                    )}
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
              {editingId
                ? `${tx("编辑行策略", "Edit Row Policy")} (${policyName})`
                : tx("新建行策略", "New Row Policy")}
            </div>
            <label className="inline-flex items-center gap-1.5 text-xs cursor-pointer">
              <input
                type="checkbox"
                checked={enabled}
                disabled={!canWrite}
                onChange={(e) => setEnabled(e.target.checked)}
                className="accent-[#2563eb]"
              />
              {tx("启用", "Enabled")}
            </label>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("策略名称", "Policy Name")} *
            </label>
            <Input
              placeholder="east_warehouses"
              value={policyName}
              disabled={!canWrite}
              onChange={(e) => setPolicyName(e.target.value)}
              data-testid="row-policy-name-input"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div className="space-y-1.5">
            <label className="text-[11px] text-muted-foreground">
              {tx(
                "适用主体（全不选表示全部非豁免主体）",
                "Target Subjects (leave empty for all non-exempt subjects)"
              )}
            </label>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
              <div className="rounded border border-border p-2 space-y-1">
                <div className="text-[11px] font-medium">
                  {tx("角色", "Roles")}
                </div>
                <div className="flex flex-wrap gap-1">
                  {roleNames.map((r) => {
                    const checked = selectedRoles.includes(r);
                    return (
                      <label
                        key={r}
                        title={r}
                        className={cn(
                          "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] border cursor-pointer",
                          checked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border text-muted-foreground"
                        )}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={!canWrite}
                          onChange={(e) =>
                            setSelectedRoles((prev) =>
                              e.target.checked
                                ? [...prev, r]
                                : prev.filter((x) => x !== r)
                            )
                          }
                          className="accent-[#2563eb]"
                        />
                        {tEnum(r)}
                      </label>
                    );
                  })}
                </div>
              </div>

              <div className="rounded border border-border p-2 space-y-1">
                <div className="text-[11px] font-medium">
                  {tx("部门", "Departments")}
                </div>
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
                          disabled={!canWrite}
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
                {tx("行过滤表达式", "Row Filter Predicate")}
              </label>
              {canWrite && (
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
              )}
            </div>

            <Textarea
              rows={3}
              disabled={!canWrite}
              placeholder="warehouse_code IN {{user.attr.warehouses}} AND status <> 'DRAFT'"
              value={predicate}
              onChange={(e) => setPredicate(e.target.value)}
              data-testid="row-policy-predicate-input"
              className="font-mono-tech text-xs"
            />
          </div>

          {canWrite && (
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
                {saving
                  ? tx("保存中…", "Saving…")
                  : editingId
                    ? tx("保存", "Save")
                    : tx("创建", "Create")}
              </Button>
            </div>
          )}
        </Card>
      </div>

      {/* Live 20-Row User Perspective Preview */}
      <Card className="p-4 space-y-3" data-testid="row-policy-preview-card">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
          <div>
            <div className="text-xs font-semibold flex items-center gap-1.5">
              <Eye className="h-3.5 w-3.5 text-xblue" />
              {tx(
                "用户视角样本预览（前 20 行）",
                "User Perspective Sample Preview (First 20 Rows)"
              )}
            </div>
            <p className="text-[11px] text-muted-foreground">
              {tx(
                "同时生效行过滤与列脱敏规则。",
                "Evaluates active row filters and column masking rules simultaneously."
              )}
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {users.length > 0 ? (
              <Select
                value={previewUserId}
                onChange={(e) => setPreviewUserId(e.target.value)}
                data-testid="row-preview-user-select"
                className="h-8 text-xs"
              >
                {users.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.username} ({u.dept_id})
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                placeholder="00000000-0000-0000-0000-000000001302"
                value={previewUserId}
                onChange={(e) => setPreviewUserId(e.target.value)}
                data-testid="row-preview-user-select"
                className="h-8 w-44 text-xs font-mono-tech"
              />
            )}

            {datasets.length > 0 ? (
              <Select
                value={previewDatasetId}
                onChange={(e) => setPreviewDatasetId(e.target.value)}
                data-testid="row-preview-dataset-select"
                className="h-8 text-xs"
              >
                {datasets.map((ds) => (
                  <option key={ds.id} value={ds.id}>
                    {ds.display_name || ds.name}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                placeholder="00000000-0000-0000-0000-000000001310"
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
              {previewing
                ? tx("预览中…", "Loading…")
                : tx("预览前 20 行", "Preview 20 Rows")}
            </Button>
          </div>
        </div>

        {previewResult && (
          <div className="space-y-2" data-testid="row-policy-preview-table">
            <div className="text-[11px] font-mono-tech text-muted-foreground">
              {tx(
                `可见记录 ${fmtNum(previewResult.row_count, 0)} 行`,
                `${fmtNum(previewResult.row_count, 0)} visible rows`
              )}
            </div>
            <div className="overflow-x-auto rounded border border-border">
              <table className="w-full text-xs border-collapse">
                <thead>
                  <tr className="border-b border-border bg-muted/40 text-left">
                    {previewResult.columns.map((col) => (
                      <th
                        key={col.name}
                        className="px-3 py-2 font-mono-tech font-medium whitespace-nowrap"
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
                        {tx("可见行数为 0", "0 visible rows")}
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
