import { useEffect, useState } from "react";
import {
  CheckCircle2,
  Code2,
  FileCheck2,
  Play,
  Plus,
  Save,
  Sparkles,
  Trash2,
  XCircle,
} from "lucide-react";
import { ApiError } from "@/lib/api";
import type { Dataset } from "@/lib/api/axhub";
import {
  axguardApi,
  type CedarPolicyRecord,
  type DepartmentRecord,
  type GrantResourceKind,
  type GuardUserRecord,
  type Sensitivity,
  type SimulateCedarPolicyResponse,
  type ValidateCedarPolicyResponse,
} from "@/lib/api/axguard";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import {
  ALL_PERMISSIONS,
  CEDAR_SNIPPET_TEMPLATES,
  GRANT_RESOURCE_KINDS,
  SENSITIVITY_OPTIONS,
} from "./constants";

interface PolicyEditorProps {
  policies: CedarPolicyRecord[];
  users: GuardUserRecord[];
  departments: DepartmentRecord[];
  datasets?: Dataset[];
  onReload: () => Promise<void>;
}

export function PolicyEditor({
  policies,
  users,
  departments,
  datasets = [],
  onReload,
}: PolicyEditorProps) {
  const { isZh, tx, tEnum } = useI18n();
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [policyName, setPolicyName] = useState("forbid_offhours_export");
  const [description, setDescription] = useState(
    CEDAR_SNIPPET_TEMPLATES[0]?.description ?? ""
  );
  const [policyText, setPolicyText] = useState(
    CEDAR_SNIPPET_TEMPLATES[0]?.policyText ?? ""
  );
  const [enabled, setEnabled] = useState(true);
  const [validation, setValidation] =
    useState<ValidateCedarPolicyResponse | null>(null);
  const [validating, setValidating] = useState(false);
  const [saving, setSaving] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);

  // Simulator state (dynamic first dataset name instead of hardcoded "demo_orders")
  const [simUserId, setSimUserId] = useState("");
  const [simAction, setSimAction] = useState<string>("DATA_READ");
  const [simResourceKind, setSimResourceKind] =
    useState<GrantResourceKind>("DATASET");
  const [simResourceRef, setSimResourceRef] = useState("");
  const [simOwnerDept, setSimOwnerDept] = useState("east");
  const [simSensitivity, setSimSensitivity] =
    useState<Sensitivity>("CONFIDENTIAL");
  const [simHour, setSimHour] = useState("14");
  const [simIp, setSimIp] = useState("10.0.1.15");
  const [simulating, setSimulating] = useState(false);
  const [simResult, setSimResult] =
    useState<SimulateCedarPolicyResponse | null>(null);

  useEffect(() => {
    if (!simUserId && users.length > 0) {
      const first = users[0];
      if (first) {
        setSimUserId(first.id);
      }
    }
  }, [users, simUserId]);

  useEffect(() => {
    if (!simResourceRef && datasets.length > 0) {
      const firstDs = datasets[0];
      if (firstDs) {
        setSimResourceRef(firstDs.name);
      }
    }
  }, [datasets, simResourceRef]);

  useEffect(() => {
    if (selectedName) {
      const found = policies.find((p) => p.name === selectedName);
      if (found) {
        setPolicyName(found.name);
        setDescription(found.description);
        setPolicyText(found.policy_text);
        setEnabled(found.enabled);
        setValidation(null);
      }
    }
  }, [selectedName, policies]);

  const handleStartNew = () => {
    setSelectedName(null);
    setPolicyName("");
    setDescription("");
    setPolicyText(CEDAR_SNIPPET_TEMPLATES[0]?.policyText ?? "");
    setEnabled(true);
    setValidation(null);
  };

  const handleValidate = async () => {
    setValidating(true);
    setBanner(null);
    try {
      const res = await axguardApi.validatePolicy(policyText);
      setValidation(res);
    } catch (err) {
      setValidation({
        valid: false,
        errors: [
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err),
        ],
      });
    } finally {
      setValidating(false);
    }
  };

  const handleSave = async () => {
    const cleanName = policyName.trim();
    if (!cleanName || !policyText.trim()) {
      setBanner(
        tx(
          "策略名称与策略正文不能为空",
          "Policy name and Cedar body are required"
        )
      );
      return;
    }
    setSaving(true);
    setBanner(null);
    try {
      const saved = await axguardApi.putPolicy(cleanName, {
        policy_text: policyText,
        description: description.trim(),
        enabled,
      });
      await onReload();
      setSelectedName(saved.name);
      setBanner(
        tx(
          `已保存策略 ${saved.name} (v${saved.version})`,
          `Saved policy ${saved.name} (v${saved.version})`
        )
      );
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (name: string) => {
    setBanner(null);
    try {
      await axguardApi.deletePolicy(name);
      if (selectedName === name) {
        handleStartNew();
      }
      await onReload();
      setBanner(tx(`已删除策略 ${name}`, `Deleted policy ${name}`));
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
  };

  const handleSimulate = async () => {
    const effectiveResourceRef =
      simResourceRef.trim() || datasets[0]?.name || "demo_orders";
    if (!simUserId.trim() || !effectiveResourceRef) {
      setBanner(
        tx(
          "需指定模拟用户与目标资源标识",
          "Target user and resource identifier are required"
        )
      );
      return;
    }
    setSimulating(true);
    setBanner(null);
    try {
      const parsedHour = Number(simHour);
      const res = await axguardApi.simulatePolicy({
        user_id: simUserId.trim(),
        action: simAction,
        resource_kind: simResourceKind,
        resource_ref: effectiveResourceRef,
        owner_dept: simOwnerDept.trim() || undefined,
        sensitivity: simSensitivity,
        hour:
          Number.isFinite(parsedHour) && parsedHour >= 0 && parsedHour <= 23
            ? parsedHour
            : undefined,
        client_ip: simIp.trim() || undefined,
      });
      setSimResult(res);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setSimulating(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="axguard-policies-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
        {/* Left: Tenant Custom Cedar Policies */}
        <Card className="xl:col-span-4 p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div className="text-xs font-semibold flex items-center gap-1.5">
              <Code2 className="h-3.5 w-3.5 text-xblue" />
              {tx("自定义 Cedar 策略", "Custom Cedar Policies")}
            </div>
            <Button
              variant="outline"
              size="sm"
              type="button"
              onClick={handleStartNew}
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新建", "New")}
            </Button>
          </div>

          {policies.length === 0 ? (
            <div className="rounded border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
              {tx("当前仅运行基线策略", "Running built-in baseline policies only")}
            </div>
          ) : (
            <div className="space-y-1.5">
              {policies.map((p) => {
                const active = p.name === selectedName;
                return (
                  <div
                    key={p.id}
                    onClick={() => setSelectedName(p.name)}
                    className={cn(
                      "flex items-center justify-between gap-2 rounded-md border px-3 py-2 text-xs cursor-pointer transition-colors",
                      active
                        ? "border-xblue bg-xblue-soft/50"
                        : "border-border hover:bg-muted/40"
                    )}
                  >
                    <div className="min-w-0">
                      <div className="font-mono-tech font-semibold truncate">
                        {p.name}
                      </div>
                      <div className="text-[11px] text-muted-foreground truncate">
                        {p.description || `v${p.version}`}
                      </div>
                    </div>
                    <div
                      className="flex items-center gap-1.5 shrink-0"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <StatusBadge status={p.enabled ? "ENABLED" : "DISABLED"} />
                      <button
                        type="button"
                        onClick={() => void handleDelete(p.name)}
                        className="text-muted-foreground hover:text-foreground p-1 cursor-pointer"
                        title={tx("删除", "Delete")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </Card>

        {/* Right: Policy Code Editor + Validator + Simulator */}
        <div className="xl:col-span-8 space-y-4">
          <Card className="p-4 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
              <div className="text-xs font-semibold">
                {tx("Cedar 策略编辑器", "Cedar Policy Editor")}
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                {CEDAR_SNIPPET_TEMPLATES.map((tpl) => (
                  <button
                    key={tpl.key}
                    type="button"
                    onClick={() => {
                      if (!selectedName) {
                        setPolicyName(tpl.suggestedName);
                      }
                      setDescription(isZh ? tpl.description : tpl.enDescription);
                      setPolicyText(tpl.policyText);
                      setValidation(null);
                    }}
                    className="inline-flex items-center gap-1 rounded border border-border bg-muted/40 hover:bg-muted px-2 py-1 text-[11px] cursor-pointer"
                  >
                    <Sparkles className="h-3 w-3 text-xblue" />
                    {isZh ? tpl.title : tpl.enTitle}
                  </button>
                ))}
              </div>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-12 gap-3">
              <div className="md:col-span-4 space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("策略名称", "Policy Name")}
                </label>
                <Input
                  placeholder="no_export"
                  value={policyName}
                  disabled={Boolean(selectedName)}
                  onChange={(e) => setPolicyName(e.target.value)}
                  data-testid="cedar-policy-name-input"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
              <div className="md:col-span-6 space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("策略说明", "Description")}
                </label>
                <Input
                  placeholder={tx("非工作时间限制数据导出", "Restrict off-hours exports")}
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>
              <div className="md:col-span-2 flex items-end pb-1">
                <label className="inline-flex items-center gap-2 text-xs cursor-pointer">
                  <input
                    type="checkbox"
                    checked={enabled}
                    onChange={(e) => setEnabled(e.target.checked)}
                    className="accent-[#2563eb]"
                  />
                  {tx("启用", "Enable")}
                </label>
              </div>
            </div>

            <div className="space-y-1.5">
              <label className="text-[11px] text-muted-foreground font-mono-tech">
                Cedar Policy
              </label>
              <Textarea
                rows={7}
                value={policyText}
                onChange={(e) => {
                  setPolicyText(e.target.value);
                  setValidation(null);
                }}
                data-testid="cedar-policy-textarea"
                className="font-mono-tech text-xs leading-relaxed"
              />
            </div>

            {validation && (
              <div
                data-testid="cedar-validation-result"
                className={cn(
                  "rounded-md border px-3 py-2.5 text-xs space-y-1",
                  validation.valid
                    ? "border-xblue/40 bg-xblue-soft/60"
                    : "border-foreground/40 bg-muted"
                )}
              >
                <div className="font-semibold flex items-center gap-1.5">
                  {validation.valid ? (
                    <>
                      <CheckCircle2 className="h-3.5 w-3.5 text-xblue" />
                      {tx(
                        "校验通过 · 符合 AxPoint Cedar Schema",
                        "Valid · Conforms to AxPoint Cedar Schema"
                      )}
                    </>
                  ) : (
                    <>
                      <XCircle className="h-3.5 w-3.5" />
                      {tx("校验未通过", "Validation failed")}
                    </>
                  )}
                </div>
                {validation.errors.length > 0 && (
                  <ul className="list-disc pl-5 font-mono-tech text-[11px] space-y-0.5">
                    {validation.errors.map((errMsg, idx) => (
                      <li key={idx}>{errMsg}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                type="button"
                disabled={validating || !policyText.trim()}
                onClick={() => void handleValidate()}
                data-testid="validate-cedar-btn"
              >
                <FileCheck2 className="h-3.5 w-3.5 text-xblue" />
                {validating
                  ? tx("校验中…", "Validating…")
                  : tx("语法校验", "Validate")}
              </Button>
              <Button
                type="button"
                disabled={saving || !policyName.trim() || !policyText.trim()}
                onClick={() => void handleSave()}
                data-testid="save-cedar-btn"
              >
                <Save className="h-3.5 w-3.5" />
                {saving
                  ? tx("保存中…", "Saving…")
                  : tx("保存策略", "Save Policy")}
              </Button>
            </div>
          </Card>

          {/* Interactive Cedar Policy Simulator */}
          <Card className="p-4 space-y-3" data-testid="cedar-simulator-card">
            <div className="border-b border-border pb-2.5">
              <div className="text-xs font-semibold">
                {tx("ABAC 策略模拟器", "ABAC Policy Simulator")}
              </div>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-4 gap-2.5">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("模拟用户", "User")}
                </label>
                {users.length > 0 ? (
                  <Select
                    value={simUserId}
                    onChange={(e) => setSimUserId(e.target.value)}
                    data-testid="sim-user-select"
                    className="h-8 w-full"
                  >
                    {users.map((u) => (
                      <option key={u.id} value={u.id}>
                        {u.username} ({u.dept_id} · {tEnum(u.clearance)})
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    placeholder="00000000-0000-0000-0000-000000000000"
                    value={simUserId}
                    onChange={(e) => setSimUserId(e.target.value)}
                    data-testid="sim-user-select"
                    className="h-8 text-xs font-mono-tech"
                  />
                )}
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("请求动作", "Action")}
                </label>
                <Select
                  value={simAction}
                  onChange={(e) => setSimAction(e.target.value)}
                  data-testid="sim-action-select"
                  className="h-8 w-full"
                >
                  {ALL_PERMISSIONS.map((perm) => (
                    <option key={perm} value={perm}>
                      {tEnum(perm)} ({perm})
                    </option>
                  ))}
                </Select>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("资源类别", "Resource Kind")}
                </label>
                <Select
                  value={simResourceKind}
                  onChange={(e) =>
                    setSimResourceKind(e.target.value as GrantResourceKind)
                  }
                  className="h-8 w-full"
                >
                  {GRANT_RESOURCE_KINDS.map((k) => (
                    <option key={k.value} value={k.value}>
                      {isZh ? k.label : k.enLabel}
                    </option>
                  ))}
                </Select>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("资源标识", "Resource ID")}
                </label>
                <Input
                  list="cedar-sim-dataset-list"
                  placeholder={datasets[0]?.name ?? "sales_orders"}
                  value={simResourceRef}
                  onChange={(e) => setSimResourceRef(e.target.value)}
                  data-testid="sim-resource-ref-input"
                  className="h-8 text-xs font-mono-tech"
                />
                <datalist id="cedar-sim-dataset-list">
                  {datasets.map((d) => (
                    <option key={d.id} value={d.name}>
                      {d.display_name}
                    </option>
                  ))}
                </datalist>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("归属部门", "Owner Department")}
                </label>
                <Input
                  list="axguard-dept-datalist"
                  value={simOwnerDept}
                  onChange={(e) => setSimOwnerDept(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
                <datalist id="axguard-dept-datalist">
                  {departments.map((d) => (
                    <option key={d.dept_id} value={d.dept_id} />
                  ))}
                </datalist>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("资源密级", "Sensitivity")}
                </label>
                <Select
                  value={simSensitivity}
                  onChange={(e) =>
                    setSimSensitivity(e.target.value as Sensitivity)
                  }
                  className="h-8 w-full"
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
                  {tx("模拟小时 (0–23)", "Hour (0–23)")}
                </label>
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={simHour}
                  onChange={(e) => setSimHour(e.target.value)}
                  data-testid="sim-hour-input"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  {tx("来源 IP", "Client IP")}
                </label>
                <Input
                  value={simIp}
                  onChange={(e) => setSimIp(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
            </div>

            <div className="flex items-center justify-between gap-3 pt-1">
              {simResult ? (
                <div
                  data-testid="cedar-simulate-result"
                  className="flex flex-wrap items-center gap-2 text-xs"
                >
                  <Badge variant={simResult.allowed ? "xblue" : "danger"}>
                    {simResult.allowed
                      ? tx("ALLOW · 允许访问", "ALLOW · Access Allowed")
                      : tx("DENY · 拒绝访问", "DENY · Access Denied")}
                  </Badge>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    {tx("命中策略", "Policies")}:{" "}
                    {simResult.determining_policies.length > 0
                      ? simResult.determining_policies.join(", ")
                      : tx("隐式拒绝", "Implicit deny")}
                  </span>
                </div>
              ) : (
                <span className="text-[11px] text-muted-foreground">
                  {tx("配置上下文参数后执行判定模拟", "Configure context parameters to simulate")}
                </span>
              )}

              <Button
                type="button"
                disabled={simulating}
                onClick={() => void handleSimulate()}
                data-testid="run-cedar-simulate-btn"
              >
                <Play className="h-3.5 w-3.5" />
                {simulating
                  ? tx("模拟中…", "Simulating…")
                  : tx("执行模拟", "Simulate")}
              </Button>
            </div>
          </Card>
        </div>
      </div>
    </div>
  );
}
