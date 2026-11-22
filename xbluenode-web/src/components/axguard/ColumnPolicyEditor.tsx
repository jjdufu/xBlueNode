import { useCallback, useEffect, useMemo, useState } from "react";
import { EyeOff, RotateCcw, Save, ShieldCheck } from "lucide-react";
import { ApiError } from "@/lib/api";
import { axhubApi, type ColumnMeta, type Dataset } from "@/lib/api/axhub";
import type { PiiCategory } from "@/lib/api/enums";
import type { BlueNodeTypeDef } from "@/lib/api/xblue";
import {
  axguardApi,
  type ColumnPolicyRecord,
  type DepartmentRecord,
  type GuardRoleRecord,
  type MaskRule,
  type PolicyTargetKind,
} from "@/lib/api/axguard";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { MASK_RULE_OPTIONS, PII_CATEGORY_LABELS } from "./constants";
import { formatSubjectSelector, resolveEffectiveMaskRule } from "./logic";

interface ColumnPolicyEditorProps {
  columnPolicies: ColumnPolicyRecord[];
  datasets: Dataset[];
  nodeTypes: BlueNodeTypeDef[];
  roles: GuardRoleRecord[];
  departments: DepartmentRecord[];
  onReload: () => Promise<void>;
}

interface UnifiedColumnItem {
  name: string;
  dataType: string;
  piiCategory: PiiCategory;
  confidenceRatio: number | null;
}

export function ColumnPolicyEditor({
  columnPolicies,
  datasets,
  nodeTypes,
  roles,
  departments,
  onReload,
}: ColumnPolicyEditorProps) {
  const { isZh, tx, tEnum } = useI18n();
  const { canAny } = usePrincipal();
  const canWrite = canAny(["GUARD_ADMIN", "DATA_ADMIN"]);

  const [targetKind, setTargetKind] = useState<PolicyTargetKind>("DATASET");
  const [targetRef, setTargetRef] = useState("");
  const [datasetColumns, setDatasetColumns] = useState<ColumnMeta[]>([]);
  const [draftRules, setDraftRules] = useState<Record<string, MaskRule>>({});
  const [draftExemptRoles, setDraftExemptRoles] = useState<
    Record<string, string[]>
  >({});
  const [draftExemptDepts, setDraftExemptDepts] = useState<
    Record<string, string[]>
  >({});
  const [customColName, setCustomColName] = useState("");
  const [customMaskRule, setCustomMaskRule] = useState<MaskRule>("MASK_PHONE");
  const [savingCol, setSavingCol] = useState<string | null>(null);
  const [banner, setBanner] = useState<string | null>(null);

  useEffect(() => {
    if (!targetRef) {
      if (targetKind === "DATASET" && datasets[0]) {
        setTargetRef(datasets[0].id);
      } else if (targetKind === "NODE_TYPE" && nodeTypes[0]) {
        setTargetRef(nodeTypes[0].name);
      }
    }
  }, [targetKind, targetRef, datasets, nodeTypes]);

  const loadDatasetCols = useCallback(async (dsId: string) => {
    if (!dsId) {
      setDatasetColumns([]);
      return;
    }
    try {
      const cols = await axhubApi.listColumns(dsId);
      setDatasetColumns(Array.isArray(cols) ? cols : []);
    } catch {
      setDatasetColumns([]);
    }
  }, []);

  useEffect(() => {
    if (targetKind === "DATASET" && targetRef) {
      void loadDatasetCols(targetRef);
    } else {
      setDatasetColumns([]);
    }
  }, [targetKind, targetRef, loadDatasetCols]);

  const policyMap = useMemo(() => {
    const map = new Map<string, ColumnPolicyRecord>();
    for (const p of columnPolicies) {
      if (p.target_kind === targetKind && p.target_ref === targetRef) {
        map.set(p.column_name, p);
      }
    }
    return map;
  }, [columnPolicies, targetKind, targetRef]);

  const unifiedColumns = useMemo<UnifiedColumnItem[]>(() => {
    const items = new Map<string, UnifiedColumnItem>();
    if (targetKind === "DATASET") {
      const ds = datasets.find((d) => d.id === targetRef);
      for (const prof of ds?.schema_json ?? []) {
        items.set(prof.name, {
          name: prof.name,
          dataType: prof.data_type,
          piiCategory: prof.pii_category ?? "NONE",
          confidenceRatio:
            typeof prof.pii_hit_ratio === "number" ? prof.pii_hit_ratio : null,
        });
      }
      for (const meta of datasetColumns) {
        const prev = items.get(meta.column_name);
        items.set(meta.column_name, {
          name: meta.column_name,
          dataType: meta.data_type,
          piiCategory: meta.pii_category ?? prev?.piiCategory ?? "NONE",
          confidenceRatio: prev?.confidenceRatio ?? null,
        });
      }
    } else {
      const nt = nodeTypes.find((n) => n.name === targetRef);
      for (const prop of nt?.properties ?? []) {
        items.set(prop.name, {
          name: prop.name,
          dataType: prop.data_type,
          piiCategory: prop.pii_category ?? "NONE",
          confidenceRatio: null,
        });
      }
    }

    for (const p of policyMap.values()) {
      if (!items.has(p.column_name)) {
        items.set(p.column_name, {
          name: p.column_name,
          dataType: "STRING",
          piiCategory: "NONE",
          confidenceRatio: null,
        });
      }
    }

    return Array.from(items.values());
  }, [targetKind, targetRef, datasets, datasetColumns, nodeTypes, policyMap]);

  const roleNames = useMemo(() => {
    const set = new Set<string>(["ADMIN", "ANALYST", "OPERATOR"]);
    for (const r of roles) {
      set.add(r.name);
    }
    return Array.from(set);
  }, [roles]);

  const handleSaveColumnPolicy = async (col: UnifiedColumnItem) => {
    if (!targetRef.trim()) return;
    const explicit = policyMap.get(col.name);
    const { effectiveRule } = resolveEffectiveMaskRule(
      col.piiCategory,
      explicit?.mask_rule
    );
    const chosenRule = draftRules[col.name] ?? effectiveRule;
    const exemptRoles =
      draftExemptRoles[col.name] ?? explicit?.exempt?.roles ?? [];
    const exemptDepts =
      draftExemptDepts[col.name] ?? explicit?.exempt?.depts ?? [];

    setSavingCol(col.name);
    setBanner(null);
    try {
      await axguardApi.putColumnPolicy({
        target_kind: targetKind,
        target_ref: targetRef.trim(),
        column_name: col.name,
        mask_rule: chosenRule,
        exempt: {
          roles: exemptRoles,
          depts: exemptDepts,
          users: explicit?.exempt?.users ?? [],
        },
      });
      await onReload();
      setBanner(
        tx(
          `已保存字段 ${col.name} 脱敏策略`,
          `Saved masking policy for ${col.name}`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
      );
    } finally {
      setSavingCol(null);
    }
  };

  const handleResetColumnPolicy = async (policyId: string, colName: string) => {
    setSavingCol(colName);
    setBanner(null);
    try {
      await axguardApi.deleteColumnPolicy(policyId);
      await onReload();
      setBanner(
        tx(
          `已恢复字段 ${colName} 默认画像脱敏规则`,
          `Restored profile default masking for ${colName}`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
      );
    } finally {
      setSavingCol(null);
    }
  };

  const handleAddManualColumnPolicy = async () => {
    if (!targetRef.trim() || !customColName.trim()) {
      setBanner(
        tx(
          "目标标识与字段名称不能为空",
          "Target identifier and field name are required"
        )
      );
      return;
    }
    setSavingCol(customColName.trim());
    setBanner(null);
    try {
      await axguardApi.putColumnPolicy({
        target_kind: targetKind,
        target_ref: targetRef.trim(),
        column_name: customColName.trim(),
        mask_rule: customMaskRule,
        exempt: {},
      });
      setCustomColName("");
      await onReload();
      setBanner(
        tx(
          `已新增字段 ${customColName.trim()} 脱敏策略`,
          `Added masking policy for ${customColName.trim()}`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
      );
    } finally {
      setSavingCol(null);
    }
  };

  return (
    <div className="space-y-4" data-testid="axguard-column-policies-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      <Card className="p-4 flex flex-wrap items-center justify-between gap-3">
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
              {tx("数据集列", "Dataset Columns")}
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
              {tx("节点属性", "Node Properties")}
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
            className="h-8 w-60 text-xs font-mono-tech"
          />
        </div>

        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span>
            {tx(
              "默认全员脱敏；原文调阅须具备豁免身份并记录审计。",
              "Masked by default; raw PII reads require exemption and emit audit logs."
            )}
          </span>
        </div>
      </Card>

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("字段 / 属性", "Column / Property")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("敏感画像与置信度", "PII Profile & Confidence")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("生效脱敏规则", "Effective Mask Rule")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("原文调阅豁免主体", "Raw Read Exemptions")}
                </th>
                <th className="px-3 py-2.5 font-medium text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {unifiedColumns.length === 0 ? (
                <tr>
                  <td
                    colSpan={5}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    {tx(
                      "当前目标暂无已发现字段",
                      "No columns discovered for current target"
                    )}
                  </td>
                </tr>
              ) : (
                unifiedColumns.map((col) => {
                  const explicit = policyMap.get(col.name);
                  const resolved = resolveEffectiveMaskRule(
                    col.piiCategory,
                    explicit?.mask_rule
                  );
                  const selectedRule =
                    draftRules[col.name] ?? resolved.effectiveRule;
                  const samplePreview =
                    MASK_RULE_OPTIONS.find((m) => m.value === selectedRule)
                      ?.sample ?? "";
                  const exemptRoles =
                    draftExemptRoles[col.name] ?? explicit?.exempt?.roles ?? [];
                  const exemptDepts =
                    draftExemptDepts[col.name] ?? explicit?.exempt?.depts ?? [];

                  return (
                    <tr key={col.name} className="hover:bg-muted/30 align-top">
                      <td className="px-3 py-2.5">
                        <div className="font-mono-tech font-semibold">
                          {col.name}
                        </div>
                        <div className="text-[11px] font-mono-tech text-muted-foreground">
                          {col.dataType}
                        </div>
                      </td>

                      <td className="px-3 py-2.5">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <Badge
                            variant={
                              col.piiCategory !== "NONE" ? "xblue" : "outline"
                            }
                          >
                            {isZh
                              ? PII_CATEGORY_LABELS[col.piiCategory]
                              : tEnum(col.piiCategory)}
                          </Badge>
                          {col.confidenceRatio !== null &&
                            col.piiCategory !== "NONE" && (
                              <span className="text-[11px] font-mono-tech text-muted-foreground">
                                {tx(
                                  `命中 ${(col.confidenceRatio * 100).toFixed(0)}%`,
                                  `Hit ${(col.confidenceRatio * 100).toFixed(0)}%`
                                )}
                              </span>
                            )}
                        </div>
                        <div className="text-[11px] text-muted-foreground mt-1">
                          {resolved.source === "EXPLICIT"
                            ? tx("管理员显式覆盖", "Explicit admin override")
                            : resolved.source === "PROFILE_DEFAULT"
                              ? tx("画像自动拦截", "Profile default mask")
                              : tx("未配置脱敏", "Unmasked")}
                        </div>
                      </td>

                      <td className="px-3 py-2.5 space-y-1">
                        <Select
                          value={selectedRule}
                          disabled={!canWrite}
                          onChange={(e) =>
                            setDraftRules((prev) => ({
                              ...prev,
                              [col.name]: e.target.value as MaskRule,
                            }))
                          }
                          className="h-8 w-full max-w-[250px] text-xs"
                        >
                          {MASK_RULE_OPTIONS.map((opt) => (
                            <option key={opt.value} value={opt.value}>
                              {isZh ? opt.label : opt.enLabel}
                            </option>
                          ))}
                        </Select>
                        <div className="text-[11px] font-mono-tech text-muted-foreground flex items-center gap-1">
                          <EyeOff className="h-3 w-3 text-xblue" />
                          <span>
                            {tx("示例", "Sample")}: {samplePreview}
                          </span>
                        </div>
                      </td>

                      <td className="px-3 py-2.5 space-y-1.5">
                        <div className="flex flex-wrap gap-1">
                          {roleNames.map((r) => {
                            const checked = exemptRoles.includes(r);
                            return (
                              <label
                                key={r}
                                title={r}
                                className={cn(
                                  "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] border cursor-pointer",
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
                                    setDraftExemptRoles((prev) => ({
                                      ...prev,
                                      [col.name]: e.target.checked
                                        ? [...exemptRoles, r]
                                        : exemptRoles.filter((x) => x !== r),
                                    }))
                                  }
                                  className="accent-[#2563eb]"
                                />
                                {tEnum(r)}
                              </label>
                            );
                          })}
                          {departments.map((d) => {
                            const checked = exemptDepts.includes(d.dept_id);
                            return (
                              <label
                                key={`dept-${d.dept_id}`}
                                className={cn(
                                  "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono-tech border cursor-pointer",
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
                                    setDraftExemptDepts((prev) => ({
                                      ...prev,
                                      [col.name]: e.target.checked
                                        ? [...exemptDepts, d.dept_id]
                                        : exemptDepts.filter(
                                            (x) => x !== d.dept_id
                                          ),
                                    }))
                                  }
                                  className="accent-[#2563eb]"
                                />
                                {tx("部门", "Dept")}:{d.dept_id}
                              </label>
                            );
                          })}
                        </div>
                        <div className="text-[11px] text-muted-foreground">
                          {formatSubjectSelector(
                            { roles: exemptRoles, depts: exemptDepts },
                            tx(
                              "仅敏感原文调阅权限持有者",
                              "PII_RAW_READ holders only"
                            )
                          )}
                        </div>
                      </td>

                      <td className="px-3 py-2.5 text-right">
                        {canWrite && (
                          <div className="inline-flex items-center gap-1.5">
                            {explicit && (
                              <Button
                                variant="outline"
                                size="sm"
                                type="button"
                                disabled={savingCol === col.name}
                                onClick={() =>
                                  void handleResetColumnPolicy(
                                    explicit.id,
                                    col.name
                                  )
                                }
                                title={tx("恢复默认", "Reset to default")}
                              >
                                <RotateCcw className="h-3 w-3" />
                              </Button>
                            )}
                            <Button
                              size="sm"
                              type="button"
                              disabled={savingCol === col.name}
                              onClick={() => void handleSaveColumnPolicy(col)}
                            >
                              <Save className="h-3 w-3" />
                              {tx("保存", "Save")}
                            </Button>
                          </div>
                        )}
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>

        {/* Manual Column/Property Policy Quick Adder */}
        {canWrite && (
          <div className="border-t border-border bg-muted/20 p-3 flex flex-wrap items-center gap-2">
            <span className="text-xs font-medium text-muted-foreground">
              {tx("自定义字段规则", "Custom Field Rule")}
            </span>
            <Input
              placeholder="phone"
              value={customColName}
              onChange={(e) => setCustomColName(e.target.value)}
              className="h-8 w-56 text-xs font-mono-tech"
            />
            <Select
              value={customMaskRule}
              onChange={(e) => setCustomMaskRule(e.target.value as MaskRule)}
              className="h-8 text-xs"
            >
              {MASK_RULE_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {isZh ? opt.label : opt.enLabel}
                </option>
              ))}
            </Select>
            <Button
              variant="outline"
              size="sm"
              type="button"
              disabled={!customColName.trim() || !targetRef.trim()}
              onClick={() => void handleAddManualColumnPolicy()}
            >
              {tx("新增策略", "Add Policy")}
            </Button>
          </div>
        )}
      </Card>
    </div>
  );
}
