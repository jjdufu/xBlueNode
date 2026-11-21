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
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
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
      setBanner(`已保存字段 ${col.name} 的脱敏策略 (${chosenRule})`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
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
      setBanner(`已清除字段 ${colName} 的显式覆盖，恢复列画像默认脱敏规则`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSavingCol(null);
    }
  };

  const handleAddManualColumnPolicy = async () => {
    if (!targetRef.trim() || !customColName.trim()) {
      setBanner("请输入目标标识与字段/属性名");
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
      setBanner(`已添加字段 ${customColName.trim()} 脱敏策略`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
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
              数据集列 (DATASET)
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
              节点属性 (NODE_TYPE)
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
              targetKind === "DATASET" ? "数据集 UUID" : "节点类型名"
            }
            value={targetRef}
            onChange={(e) => setTargetRef(e.target.value)}
            className="h-8 w-60 text-xs font-mono-tech"
          />
        </div>

        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
          <span>
            默认全员脱敏；仅具备 PII_RAW_READ 或列入豁免名单且显式请求 reveal_pii: true 时记审计返原文
          </span>
        </div>
      </Card>

      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 font-medium">字段 / 属性名</th>
                <th className="px-3 py-2.5 font-medium">Presidio 画像识别与置信度</th>
                <th className="px-3 py-2.5 font-medium">生效脱敏规则 (MaskRule)</th>
                <th className="px-3 py-2.5 font-medium">
                  原文调阅豁免主体 (reveal_pii)
                </th>
                <th className="px-3 py-2.5 font-medium text-right">操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {unifiedColumns.length === 0 ? (
                <tr>
                  <td
                    colSpan={5}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    当前目标暂无已发现字段，可使用下方表单直接新增字段脱敏规则
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
                            {PII_CATEGORY_LABELS[col.piiCategory]}
                          </Badge>
                          {col.confidenceRatio !== null &&
                            col.piiCategory !== "NONE" && (
                              <span className="text-[11px] font-mono-tech text-muted-foreground">
                                命中 {(col.confidenceRatio * 100).toFixed(0)}%
                              </span>
                            )}
                        </div>
                        <div className="text-[11px] text-muted-foreground mt-1">
                          {resolved.source === "EXPLICIT"
                            ? "来源：管理员显式策略覆盖"
                            : resolved.source === "PROFILE_DEFAULT"
                              ? "来源：列画像自动默认拦截 (§13.7)"
                              : "未配置脱敏"}
                        </div>
                      </td>

                      <td className="px-3 py-2.5 space-y-1">
                        <select
                          value={selectedRule}
                          onChange={(e) =>
                            setDraftRules((prev) => ({
                              ...prev,
                              [col.name]: e.target.value as MaskRule,
                            }))
                          }
                          className="h-8 w-full max-w-[250px] rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                        >
                          {MASK_RULE_OPTIONS.map((opt) => (
                            <option key={opt.value} value={opt.value}>
                              {opt.label}
                            </option>
                          ))}
                        </select>
                        <div className="text-[11px] font-mono-tech text-muted-foreground flex items-center gap-1">
                          <EyeOff className="h-3 w-3 text-xblue" />
                          样例输出: {samplePreview}
                        </div>
                      </td>

                      <td className="px-3 py-2.5 space-y-1.5">
                        <div className="flex flex-wrap gap-1">
                          {roleNames.map((r) => {
                            const checked = exemptRoles.includes(r);
                            return (
                              <label
                                key={r}
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
                                {r}
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
                                部门:{d.dept_id}
                              </label>
                            );
                          })}
                        </div>
                        <div className="text-[11px] text-muted-foreground">
                          {formatSubjectSelector(
                            { roles: exemptRoles, depts: exemptDepts },
                            "仅限 PII_RAW_READ 权限持有者"
                          )}
                        </div>
                      </td>

                      <td className="px-3 py-2.5 text-right">
                        <div className="inline-flex items-center gap-1.5">
                          {explicit && (
                            <Button
                              variant="outline"
                              type="button"
                              disabled={savingCol === col.name}
                              onClick={() =>
                                void handleResetColumnPolicy(
                                  explicit.id,
                                  col.name
                                )
                              }
                              className="px-2 py-1 text-[11px]"
                              title="恢复画像默认规则"
                            >
                              <RotateCcw className="h-3 w-3" />
                            </Button>
                          )}
                          <Button
                            type="button"
                            disabled={savingCol === col.name}
                            onClick={() => void handleSaveColumnPolicy(col)}
                            className="px-2.5 py-1 text-[11px]"
                          >
                            <Save className="h-3 w-3" />
                            保存
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

        {/* Manual Column/Property Policy Quick Adder */}
        <div className="border-t border-border bg-muted/20 p-3 flex flex-wrap items-center gap-2">
          <span className="text-xs font-medium text-muted-foreground">
            按列名直接新增脱敏规则：
          </span>
          <Input
            placeholder="字段或节点属性名（如 phone / id_card）"
            value={customColName}
            onChange={(e) => setCustomColName(e.target.value)}
            className="h-8 w-56 text-xs font-mono-tech"
          />
          <select
            value={customMaskRule}
            onChange={(e) => setCustomMaskRule(e.target.value as MaskRule)}
            className="h-8 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
          >
            {MASK_RULE_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
          <Button
            variant="outline"
            type="button"
            disabled={!customColName.trim() || !targetRef.trim()}
            onClick={() => void handleAddManualColumnPolicy()}
          >
            新增字段策略
          </Button>
        </div>
      </Card>
    </div>
  );
}
