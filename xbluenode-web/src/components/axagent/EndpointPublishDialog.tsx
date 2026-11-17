import { useState } from "react";
import { z } from "zod";
import { ArrowLeft, ArrowRight, Check, Copy, Globe, X } from "lucide-react";
import type { EndpointKind, Environment, Permission } from "@/lib/api/enums";
import {
  createEndpoint,
  fetchEndpointSkillManifest,
  type AgentRecord,
  type EndpointSpec,
  type SkillManifest,
} from "@/lib/api/axagent";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

const PublishEndpointSchema = z.object({
  slug: z
    .string()
    .trim()
    .regex(/^[a-z0-9-]{3,64}$/, "端点标识 slug 需为 3~64 位小写字母、数字或连字符"),
  kind: z.enum(["METRIC", "SAVED_QUERY", "OBJECT_TYPE", "FLOW", "AGENT"]),
  target_id: z.string().trim().min(1, "目标资源标识不能为空"),
  title: z.string().trim().min(1, "端点名称不能为空"),
  description: z.string().max(500),
  environment: z.enum(["TEST", "PROD"]),
  required_scope: z.string().min(1),
  rate_limit_rpm: z.number().int().min(1).max(6000),
  cache_ttl_s: z.number().int().min(0).max(86400),
  expose_mcp: z.boolean(),
  show_as_app: z.boolean(),
});

const ENDPOINT_KINDS: ReadonlyArray<{
  kind: EndpointKind;
  label: string;
  hint: string;
  placeholder: string;
}> = [
  {
    kind: "METRIC",
    label: "语义指标 (METRIC)",
    hint: "自动推导维度、过滤与时间粒度 JSON Schema",
    placeholder: "paid_amount",
  },
  {
    kind: "SAVED_QUERY",
    label: "保存查询 (SAVED_QUERY)",
    hint: "按保存查询参数模板发布为受控 REST 接口",
    placeholder: "00000000-0000-0000-0000-000000000401",
  },
  {
    kind: "OBJECT_TYPE",
    label: "主图对象类型 (OBJECT_TYPE)",
    hint: "按主图 NodeType 属性、指标与 1 跳关系发布领域对象接口",
    placeholder: "Warehouse",
  },
  {
    kind: "FLOW",
    label: "编排流水线 (FLOW)",
    hint: "异步启动 TEST/PROD 流程部署并返回 run_id",
    placeholder: "00000000-0000-0000-0000-000000000701",
  },
  {
    kind: "AGENT",
    label: "智能体对话 (AGENT)",
    hint: "将已发布智能体暴露为标准 API / Skill 能力",
    placeholder: "00000000-0000-0000-0000-000000000901",
  },
];

const SCOPES: readonly Permission[] = [
  "AGENT_RUN",
  "DATA_READ",
  "FLOW_RUN",
  "SIM_RUN",
];

export interface EndpointPublishDialogProps {
  open: boolean;
  agents: AgentRecord[];
  metricNames?: string[];
  objectTypeNames?: string[];
  onClose: () => void;
  onCreated: (created: EndpointSpec) => Promise<void> | void;
}

export function EndpointPublishDialog({
  open,
  agents,
  metricNames = [],
  objectTypeNames = [],
  onClose,
  onCreated,
}: EndpointPublishDialogProps) {
  const [step, setStep] = useState<1 | 2 | 3>(1);
  const [kind, setKind] = useState<EndpointKind>("METRIC");
  const [targetId, setTargetId] = useState("");
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [environment, setEnvironment] = useState<Environment>("TEST");
  const [requiredScope, setRequiredScope] = useState<Permission>("AGENT_RUN");
  const [rateLimitRpm, setRateLimitRpm] = useState("60");
  const [cacheTtlS, setCacheTtlS] = useState("0");
  const [exposeMcp, setExposeMcp] = useState(true);
  const [showAsApp, setShowAsApp] = useState(true);
  const [customSchemaText, setCustomSchemaText] = useState("");

  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [createdEndpoint, setCreatedEndpoint] = useState<EndpointSpec | null>(
    null
  );
  const [createdSkill, setCreatedSkill] = useState<SkillManifest | null>(null);
  const [copiedField, setCopiedField] = useState<"curl" | "skill" | null>(null);

  if (!open) {
    return null;
  }

  const activeKindMeta =
    ENDPOINT_KINDS.find((k) => k.kind === kind) ?? ENDPOINT_KINDS[0];

  const handleNextStep1 = () => {
    if (!targetId.trim()) {
      setErrorMsg("请填写或选择目标资源标识 (target_id)");
      return;
    }
    setErrorMsg(null);
    if (!slug.trim()) {
      const autoSlug = targetId
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9-]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 48);
      if (autoSlug.length >= 3) {
        setSlug(autoSlug);
      }
    }
    if (!title.trim()) {
      setTitle(`${kind} · ${targetId.trim()}`);
    }
    setStep(2);
  };

  const handlePublish = async () => {
    setErrorMsg(null);
    const parsed = PublishEndpointSchema.safeParse({
      slug,
      kind,
      target_id: targetId,
      title,
      description,
      environment,
      required_scope: requiredScope,
      rate_limit_rpm: Number(rateLimitRpm),
      cache_ttl_s: Number(cacheTtlS),
      expose_mcp: exposeMcp,
      show_as_app: showAsApp,
    });
    if (!parsed.success) {
      setErrorMsg(parsed.error.issues[0]?.message ?? "参数格式校验未通过");
      return;
    }

    let customSchema: Record<string, unknown> | null = null;
    if (customSchemaText.trim().length > 0) {
      try {
        const obj: unknown = JSON.parse(customSchemaText.trim());
        if (typeof obj !== "object" || obj === null || Array.isArray(obj)) {
          setErrorMsg("自定义输入 Schema 必须为 JSON 对象");
          return;
        }
        customSchema = obj as Record<string, unknown>;
      } catch {
        setErrorMsg("自定义输入 Schema JSON 解析失败");
        return;
      }
    }

    setSubmitting(true);
    try {
      const created = await createEndpoint({
        slug: parsed.data.slug,
        environment: parsed.data.environment,
        kind: parsed.data.kind,
        target_id: parsed.data.target_id,
        title: parsed.data.title,
        description: parsed.data.description,
        input_schema: customSchema,
        required_scope: requiredScope,
        rate_limit_rpm: parsed.data.rate_limit_rpm,
        cache_ttl_s: parsed.data.cache_ttl_s,
        expose_mcp: parsed.data.expose_mcp,
        show_as_app: parsed.data.show_as_app,
        enabled: true,
      });
      setCreatedEndpoint(created);
      try {
        const skill = await fetchEndpointSkillManifest(created.id);
        setCreatedSkill(skill);
      } catch {
        setCreatedSkill({
          name: created.slug.replace(/-/g, "_"),
          description: created.description || created.title,
          input_schema: created.input_schema,
          invoke: {
            method: "POST",
            url: `/api/published/${created.environment.toLowerCase()}/${created.tenant_id}/${created.slug}`,
          },
          auth: "bearer",
        });
      }
      await onCreated(created);
      setStep(3);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "发布能力端点失败");
    } finally {
      setSubmitting(false);
    }
  };

  const copySnippet = async (text: string, field: "curl" | "skill") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedField(field);
      setTimeout(() => setCopiedField(null), 1800);
    } catch {
      // Ignore in restricted environments.
    }
  };

  const origin =
    typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:9995";
  const curlSnippet = createdEndpoint
    ? `curl -X POST "${origin}/api/published/${createdEndpoint.environment.toLowerCase()}/${createdEndpoint.tenant_id}/${createdEndpoint.slug}" \\\n  -H "Authorization: Bearer axp_<YOUR_PAT_TOKEN>" \\\n  -H "Content-Type: application/json" \\\n  -d '{}'`
    : "";

  return (
    <div
      data-testid="endpoint-publish-dialog"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    >
      <Card className="w-full max-w-2xl max-h-[90vh] overflow-y-auto p-5 space-y-4 shadow-lg">
        <div className="flex items-center justify-between border-b border-border pb-3">
          <div className="flex items-center gap-2">
            <Globe className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">发布平台能力端点</h2>
              <p className="text-[11px] text-muted-foreground">
                步骤 {step} / 3 ·{" "}
                {step === 1
                  ? "选择能力类型与目标资源"
                  : step === 2
                    ? "配置环境、速率限制与 MCP/应用暴露"
                    : "发布完成 · 复制调用凭证清单"}
              </p>
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            onClick={onClose}
            className="h-7 w-7 p-0"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>

        {errorMsg ? (
          <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
            {errorMsg}
          </div>
        ) : null}

        {step === 1 ? (
          <div className="space-y-4 text-xs">
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              {ENDPOINT_KINDS.map((item) => {
                const selected = item.kind === kind;
                return (
                  <button
                    key={item.kind}
                    type="button"
                    data-testid={`publish-kind-${item.kind}`}
                    onClick={() => {
                      setKind(item.kind);
                      setTargetId("");
                    }}
                    className={cn(
                      "text-left rounded-md border p-3 transition-colors cursor-pointer space-y-1",
                      selected
                        ? "border-xblue bg-xblue-soft/40"
                        : "border-border bg-background hover:bg-muted/50"
                    )}
                  >
                    <div className="font-semibold text-foreground">
                      {item.label}
                    </div>
                    <div className="text-[11px] text-muted-foreground">
                      {item.hint}
                    </div>
                  </button>
                );
              })}
            </div>

            <div className="space-y-1.5">
              <label className="text-[11px] font-medium text-muted-foreground">
                目标资源标识 (target_id)
              </label>

              {kind === "AGENT" && agents.length > 0 ? (
                <select
                  value={targetId}
                  onChange={(e) => setTargetId(e.target.value)}
                  className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs"
                >
                  <option value="">-- 选择已发布智能体 --</option>
                  {agents.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name} (v{a.version})
                    </option>
                  ))}
                </select>
              ) : kind === "METRIC" && metricNames.length > 0 ? (
                <select
                  value={targetId}
                  onChange={(e) => setTargetId(e.target.value)}
                  className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                >
                  <option value="">-- 选择语义指标或下方手动输入 --</option>
                  {metricNames.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              ) : kind === "OBJECT_TYPE" && objectTypeNames.length > 0 ? (
                <select
                  value={targetId}
                  onChange={(e) => setTargetId(e.target.value)}
                  className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                >
                  <option value="">-- 选择主图对象类型或下方手动输入 --</option>
                  {objectTypeNames.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              ) : null}

              <Input
                data-testid="publish-target-input"
                value={targetId}
                onChange={(e) => setTargetId(e.target.value)}
                placeholder={activeKindMeta?.placeholder ?? "target_id"}
                className="font-mono-tech text-xs"
              />
            </div>

            <div className="flex justify-end pt-2">
              <Button
                type="button"
                data-testid="publish-step1-next-btn"
                onClick={handleNextStep1}
              >
                下一步：配置端点策略
                <ArrowRight className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
        ) : step === 2 ? (
          <div className="space-y-3.5 text-xs">
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  端点 URL 标识 (slug)
                </label>
                <Input
                  data-testid="publish-slug-input"
                  value={slug}
                  onChange={(e) => setSlug(e.target.value)}
                  placeholder="paid-amount-by-status"
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  显示名称 (title)
                </label>
                <Input
                  data-testid="publish-title-input"
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  placeholder="已支付金额（按状态）"
                  className="text-xs"
                />
              </div>
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                能力说明 (description)
              </label>
              <Input
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="对外提供状态维度的实时或快照聚合查询"
                className="text-xs"
              />
            </div>

            <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  发布环境
                </label>
                <select
                  value={environment}
                  onChange={(e) =>
                    setEnvironment(e.target.value as Environment)
                  }
                  className="h-9 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                >
                  <option value="TEST">TEST (跟随最新)</option>
                  <option value="PROD">PROD (版本快照)</option>
                </select>
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  要求令牌权限
                </label>
                <select
                  value={requiredScope}
                  onChange={(e) =>
                    setRequiredScope(e.target.value as Permission)
                  }
                  className="h-9 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                >
                  {SCOPES.map((s) => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  限流 (RPM)
                </label>
                <Input
                  type="number"
                  min={1}
                  value={rateLimitRpm}
                  onChange={(e) => setRateLimitRpm(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  缓存 TTL (秒)
                </label>
                <Input
                  type="number"
                  min={0}
                  value={cacheTtlS}
                  onChange={(e) => setCacheTtlS(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
            </div>

            <div className="flex flex-wrap gap-4 pt-1">
              <label className="inline-flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={exposeMcp}
                  onChange={(e) => setExposeMcp(e.target.checked)}
                  className="accent-[#2563eb]"
                />
                <span>同时暴露到 /api/mcp 工具列表 (expose_mcp)</span>
              </label>
              <label className="inline-flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={showAsApp}
                  onChange={(e) => setShowAsApp(e.target.checked)}
                  className="accent-[#2563eb]"
                />
                <span>在「应用」页签渲染为查询即应用卡片 (show_as_app)</span>
              </label>
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                自定义输入 JSON Schema（留空由平台按目标定义自动生成）
              </label>
              <Textarea
                value={customSchemaText}
                onChange={(e) => setCustomSchemaText(e.target.value)}
                rows={2}
                placeholder="留空自动生成 Draft 2020-12 JSON Schema"
                className="font-mono-tech text-xs"
              />
            </div>

            <div className="flex items-center justify-between pt-2">
              <Button
                type="button"
                variant="outline"
                onClick={() => setStep(1)}
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                上一步
              </Button>
              <Button
                type="button"
                data-testid="publish-submit-btn"
                disabled={submitting}
                onClick={() => void handlePublish()}
              >
                {submitting ? "正在发布…" : "确认发布端点"}
              </Button>
            </div>
          </div>
        ) : createdEndpoint ? (
          <div className="space-y-4 text-xs" data-testid="publish-step3-done">
            <div className="flex items-center gap-2 rounded-md border border-xblue/40 bg-xblue-soft/40 p-3">
              <Badge variant="xblue">{createdEndpoint.environment}</Badge>
              <span className="font-semibold">{createdEndpoint.title}</span>
              <span className="font-mono-tech text-muted-foreground">
                ({createdEndpoint.slug})
              </span>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <span className="font-medium">cURL 调用示例</span>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void copySnippet(curlSnippet, "curl")}
                  className="h-7 px-2.5"
                >
                  {copiedField === "curl" ? (
                    <Check className="h-3.5 w-3.5 text-xblue" />
                  ) : (
                    <Copy className="h-3.5 w-3.5" />
                  )}
                  复制 cURL
                </Button>
              </div>
              <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto">
                {curlSnippet}
              </pre>
            </div>

            {createdSkill ? (
              <div className="space-y-1.5">
                <div className="flex items-center justify-between">
                  <span className="font-medium">Skill 清单 (skill.json)</span>
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() =>
                      void copySnippet(
                        JSON.stringify(createdSkill, null, 2),
                        "skill"
                      )
                    }
                    className="h-7 px-2.5"
                  >
                    {copiedField === "skill" ? (
                      <Check className="h-3.5 w-3.5 text-xblue" />
                    ) : (
                      <Copy className="h-3.5 w-3.5" />
                    )}
                    复制 Skill JSON
                  </Button>
                </div>
                <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-44">
                  {JSON.stringify(createdSkill, null, 2)}
                </pre>
              </div>
            ) : null}

            <div className="flex justify-end">
              <Button type="button" onClick={onClose}>
                完成并关闭
              </Button>
            </div>
          </div>
        ) : null}
      </Card>
    </div>
  );
}
