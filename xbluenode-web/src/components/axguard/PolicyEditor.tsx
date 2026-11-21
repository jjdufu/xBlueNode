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
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
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
  onReload: () => Promise<void>;
}

export function PolicyEditor({
  policies,
  users,
  departments,
  onReload,
}: PolicyEditorProps) {
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [policyName, setPolicyName] = useState("forbid_offhours_export");
  const [description, setDescription] = useState(
    "非工作时间禁止执行数据出品动作"
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

  // Simulator state
  const [simUserId, setSimUserId] = useState("");
  const [simAction, setSimAction] = useState<string>("DATA_READ");
  const [simResourceKind, setSimResourceKind] =
    useState<GrantResourceKind>("DATASET");
  const [simResourceRef, setSimResourceRef] = useState("demo_orders");
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
      setBanner("请填写策略名称与 Cedar 策略正文");
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
      setBanner(`已保存并生效 Cedar 策略 ${saved.name} (v${saved.version})`);
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
      setBanner(`已删除 Cedar 策略 ${name}`);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    }
  };

  const handleSimulate = async () => {
    if (!simUserId.trim() || !simResourceRef.trim()) {
      setBanner("策略模拟需指定目标用户与资源标识");
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
        resource_ref: simResourceRef.trim(),
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
            <div>
              <div className="text-xs font-semibold flex items-center gap-1.5">
                <Code2 className="h-3.5 w-3.5 text-xblue" />
                租户自定义 Cedar 策略
              </div>
              <p className="text-[11px] text-muted-foreground">
                基线 5 条 permit 策略已内置固化；自定义 forbid 策略拥有最高否决优先级。
              </p>
            </div>
            <Button variant="outline" type="button" onClick={handleStartNew}>
              <Plus className="h-3.5 w-3.5" />
              新建
            </Button>
          </div>

          {policies.length === 0 ? (
            <div className="rounded border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
              当前仅运行平台基线策略，可从右侧模板快速添加自定义策略
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
                      <Badge variant={p.enabled ? "xblue" : "outline"}>
                        {p.enabled ? "启用" : "停用"}
                      </Badge>
                      <button
                        type="button"
                        onClick={() => void handleDelete(p.name)}
                        className="text-muted-foreground hover:text-foreground p-1 cursor-pointer"
                        title="删除策略"
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
                Cedar 策略编辑器（AxPoint Schema 强类型校验）
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
                      setDescription(tpl.description);
                      setPolicyText(tpl.policyText);
                      setValidation(null);
                    }}
                    className="inline-flex items-center gap-1 rounded border border-border bg-muted/40 hover:bg-muted px-2 py-1 text-[11px] cursor-pointer"
                  >
                    <Sparkles className="h-3 w-3 text-xblue" />
                    {tpl.title}
                  </button>
                ))}
              </div>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-12 gap-3">
              <div className="md:col-span-4 space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  策略唯一名称 (name)
                </label>
                <Input
                  placeholder="如 no_export"
                  value={policyName}
                  disabled={Boolean(selectedName)}
                  onChange={(e) => setPolicyName(e.target.value)}
                  data-testid="cedar-policy-name-input"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
              <div className="md:col-span-6 space-y-1">
                <label className="text-[11px] text-muted-foreground">策略说明</label>
                <Input
                  placeholder="简要说明管控意图与适用范围"
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
                  立即启用
                </label>
              </div>
            </div>

            <div className="space-y-1.5">
              <label className="text-[11px] text-muted-foreground font-mono-tech">
                Cedar Policy Text
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
                      校验通过 · 符合 AxPoint Cedar Schema
                    </>
                  ) : (
                    <>
                      <XCircle className="h-3.5 w-3.5" />
                      语法或 Schema 校验未通过
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
                {validating ? "校验中..." : "语法与 Schema 校验"}
              </Button>
              <Button
                type="button"
                disabled={saving || !policyName.trim() || !policyText.trim()}
                onClick={() => void handleSave()}
                data-testid="save-cedar-btn"
              >
                <Save className="h-3.5 w-3.5" />
                {saving ? "保存中..." : "保存策略"}
              </Button>
            </div>
          </Card>

          {/* Interactive Cedar Policy Simulator */}
          <Card className="p-4 space-y-3" data-testid="cedar-simulator-card">
            <div className="border-b border-border pb-2.5">
              <div className="text-xs font-semibold">
                ABAC 策略沙盘模拟器（POST /api/axguard/policies/simulate）
              </div>
              <p className="text-[11px] text-muted-foreground">
                输入主体、动作、资源密级及时间/IP 上下文，实时检视基线 + 跨部门授权 + 自定义策略的综合判定结果。
              </p>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-4 gap-2.5">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">模拟用户</label>
                {users.length > 0 ? (
                  <select
                    value={simUserId}
                    onChange={(e) => setSimUserId(e.target.value)}
                    data-testid="sim-user-select"
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {users.map((u) => (
                      <option key={u.id} value={u.id}>
                        {u.username} ({u.dept_id} · {u.clearance})
                      </option>
                    ))}
                  </select>
                ) : (
                  <Input
                    placeholder="用户 UUID"
                    value={simUserId}
                    onChange={(e) => setSimUserId(e.target.value)}
                    data-testid="sim-user-select"
                    className="h-8 text-xs font-mono-tech"
                  />
                )}
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">请求动作</label>
                <select
                  value={simAction}
                  onChange={(e) => setSimAction(e.target.value)}
                  data-testid="sim-action-select"
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                >
                  {ALL_PERMISSIONS.map((perm) => (
                    <option key={perm} value={perm}>
                      {perm}
                    </option>
                  ))}
                </select>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">资源类别</label>
                <select
                  value={simResourceKind}
                  onChange={(e) =>
                    setSimResourceKind(e.target.value as GrantResourceKind)
                  }
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                >
                  {GRANT_RESOURCE_KINDS.map((k) => (
                    <option key={k.value} value={k.value}>
                      {k.label}
                    </option>
                  ))}
                </select>
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  资源标识 (resource_ref)
                </label>
                <Input
                  value={simResourceRef}
                  onChange={(e) => setSimResourceRef(e.target.value)}
                  data-testid="sim-resource-ref-input"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>

              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  资源属主部门
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
                <label className="text-[11px] text-muted-foreground">资源密级</label>
                <select
                  value={simSensitivity}
                  onChange={(e) =>
                    setSimSensitivity(e.target.value as Sensitivity)
                  }
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
                <label className="text-[11px] text-muted-foreground">
                  模拟小时 (0-23)
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
                  客户端 IP
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
                  <Badge variant={simResult.allowed ? "xblue" : "neutral"}>
                    {simResult.allowed ? "ALLOW · 允许访问" : "DENY · 拒绝访问"}
                  </Badge>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    命中策略:{" "}
                    {simResult.determining_policies.length > 0
                      ? simResult.determining_policies.join(", ")
                      : "默认隐式拒绝 (implicit deny)"}
                  </span>
                </div>
              ) : (
                <span className="text-[11px] text-muted-foreground">
                  点击右侧按钮运行策略判定模拟
                </span>
              )}

              <Button
                type="button"
                disabled={simulating}
                onClick={() => void handleSimulate()}
                data-testid="run-cedar-simulate-btn"
              >
                <Play className="h-3.5 w-3.5" />
                {simulating ? "推演判定中..." : "执行策略模拟"}
              </Button>
            </div>
          </Card>
        </div>
      </div>
    </div>
  );
}
