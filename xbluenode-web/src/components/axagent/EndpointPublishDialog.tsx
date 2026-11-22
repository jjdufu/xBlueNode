import { useState } from "react";
import { z } from "zod";
import { ArrowLeft, ArrowRight, Check, Copy, Globe } from "lucide-react";
import type { EndpointKind, Environment, Permission } from "@/lib/api/enums";
import {
  createEndpoint,
  fetchEndpointSkillManifest,
  type AgentRecord,
  type EndpointSpec,
  type SkillManifest,
} from "@/lib/api/axagent";
import {
  Button,
  Input,
  Modal,
  Select,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

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
  const { tx, tEnum } = useI18n();
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

  const endpointKinds: ReadonlyArray<{
    kind: EndpointKind;
    label: string;
    hint: string;
    placeholder: string;
  }> = [
    {
      kind: "METRIC",
      label: tEnum("METRIC"),
      hint: tx(
        "自动推导维度、过滤条件与时间粒度 JSON Schema",
        "Auto-derives dimension, filter, and time-grain JSON Schema"
      ),
      placeholder: "paid_amount",
    },
    {
      kind: "SAVED_QUERY",
      label: tEnum("SAVED_QUERY"),
      hint: tx(
        "将已存查询参数模板发布为受控 REST 接口",
        "Publishes a saved query parameter template as a controlled REST API"
      ),
      placeholder: "00000000-0000-0000-0000-000000000401",
    },
    {
      kind: "OBJECT_TYPE",
      label: tEnum("OBJECT_TYPE"),
      hint: tx(
        "暴露主图节点属性、指标与一跳关联查询接口",
        "Exposes main-graph node attributes, metrics, and 1-hop links"
      ),
      placeholder: "Warehouse",
    },
    {
      kind: "FLOW",
      label: tEnum("FLOW"),
      hint: tx(
        "异步触发编排部署并返回运行实例标识",
        "Asynchronously triggers pipeline deployment and returns run_id"
      ),
      placeholder: "00000000-0000-0000-0000-000000000701",
    },
    {
      kind: "AGENT",
      label: tEnum("AGENT"),
      hint: tx(
        "将已发布智能体暴露为标准 API 与 Skill 能力",
        "Exposes a published agent as a standard API and Skill"
      ),
      placeholder: "00000000-0000-0000-0000-000000000901",
    },
  ];

  const activeKindMeta =
    endpointKinds.find((k) => k.kind === kind) ?? endpointKinds[0];

  const handleNextStep1 = () => {
    if (!targetId.trim()) {
      setErrorMsg(tx("目标资源标识不能为空", "Target resource ID is required"));
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
    const publishSchema = z.object({
      slug: z
        .string()
        .trim()
        .regex(
          /^[a-z0-9-]{3,64}$/,
          tx(
            "端点标识需为 3~64 位小写字母、数字或连字符",
            "Slug must be 3-64 lowercase letters, digits, or hyphens"
          )
        ),
      kind: z.enum(["METRIC", "SAVED_QUERY", "OBJECT_TYPE", "FLOW", "AGENT"]),
      target_id: z
        .string()
        .trim()
        .min(1, tx("目标资源标识不能为空", "Target ID is required")),
      title: z
        .string()
        .trim()
        .min(1, tx("端点名称不能为空", "Title is required")),
      description: z.string().max(500),
      environment: z.enum(["TEST", "PROD"]),
      required_scope: z.string().min(1),
      rate_limit_rpm: z.number().int().min(1).max(6000),
      cache_ttl_s: z.number().int().min(0).max(86400),
      expose_mcp: z.boolean(),
      show_as_app: z.boolean(),
    });

    const parsed = publishSchema.safeParse({
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
      setErrorMsg(
        parsed.error.issues[0]?.message ??
          tx("参数格式有误", "Invalid parameters")
      );
      return;
    }

    let customSchema: Record<string, unknown> | null = null;
    if (customSchemaText.trim().length > 0) {
      try {
        const obj: unknown = JSON.parse(customSchemaText.trim());
        if (typeof obj !== "object" || obj === null || Array.isArray(obj)) {
          setErrorMsg(
            tx("自定义输入 Schema 须为 JSON 对象", "Custom schema must be a JSON object")
          );
          return;
        }
        customSchema = obj as Record<string, unknown>;
      } catch {
        setErrorMsg(
          tx("自定义输入 Schema JSON 解析失败", "Invalid custom schema JSON")
        );
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("发布失败", "Failed to publish endpoint")
      );
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
    <Modal
      open={open}
      onClose={onClose}
      maxWidthClass="max-w-2xl"
      testId="endpoint-publish-dialog"
      title={
        <>
          <Globe className="h-4 w-4 text-xblue shrink-0" />
          <span>
            {tx(
              `发布能力端点 (${step}/3)`,
              `Publish Endpoint (${step}/3)`
            )}
          </span>
        </>
      }
    >
      {errorMsg ? (
        <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
          {errorMsg}
        </div>
      ) : null}

      {step === 1 ? (
        <div className="space-y-4 text-xs">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {endpointKinds.map((item) => {
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
              {tx("目标资源标识", "Target Resource ID")}
            </label>

            {kind === "AGENT" && agents.length > 0 ? (
              <Select
                value={targetId}
                onChange={(e) => setTargetId(e.target.value)}
                className="w-full"
              >
                <option value="">{tx("指定智能体", "Target Agent")}</option>
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} (v{a.version})
                  </option>
                ))}
              </Select>
            ) : kind === "METRIC" && metricNames.length > 0 ? (
              <Select
                value={targetId}
                onChange={(e) => setTargetId(e.target.value)}
                className="w-full font-mono-tech"
              >
                <option value="">{tx("指定语义指标", "Target Metric")}</option>
                {metricNames.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            ) : kind === "OBJECT_TYPE" && objectTypeNames.length > 0 ? (
              <Select
                value={targetId}
                onChange={(e) => setTargetId(e.target.value)}
                className="w-full font-mono-tech"
              >
                <option value="">
                  {tx("指定对象类型", "Target Object Type")}
                </option>
                {objectTypeNames.map((t) => (
                  <option key={t} value={t}>
                    {t}
                  </option>
                ))}
              </Select>
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
              {tx("下一步", "Next")}
              <ArrowRight className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      ) : step === 2 ? (
        <div className="space-y-3.5 text-xs">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("端点标识", "Endpoint Slug")}
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
                {tx("显示名称", "Title")}
              </label>
              <Input
                data-testid="publish-title-input"
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={tx("已支付金额", "Paid Amount")}
                className="text-xs"
              />
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("能力说明", "Description")}
            </label>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={tx("多维状态实时或快照聚合查询", "Aggregated status query")}
              className="text-xs"
            />
          </div>

          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("发布环境", "Environment")}
              </label>
              <Select
                value={environment}
                onChange={(e) =>
                  setEnvironment(e.target.value as Environment)
                }
                className="w-full"
              >
                <option value="TEST">{tEnum("TEST")}</option>
                <option value="PROD">{tEnum("PROD")}</option>
              </Select>
            </div>
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("所需权限", "Required Scope")}
              </label>
              <Select
                value={requiredScope}
                onChange={(e) =>
                  setRequiredScope(e.target.value as Permission)
                }
                className="w-full"
              >
                {SCOPES.map((s) => (
                  <option key={s} value={s} title={s}>
                    {tEnum(s)}
                  </option>
                ))}
              </Select>
            </div>
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("限流 (RPM)", "Rate Limit (RPM)")}
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
                {tx("缓存 TTL（秒）", "Cache TTL (s)")}
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
              <span>{tx("暴露为 MCP 工具", "Expose via MCP")}</span>
            </label>
            <label className="inline-flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={showAsApp}
                onChange={(e) => setShowAsApp(e.target.checked)}
                className="accent-[#2563eb]"
              />
              <span>{tx("显示为查询应用", "Show in Apps Gallery")}</span>
            </label>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("自定义输入 JSON Schema", "Custom Input JSON Schema")}
            </label>
            <Textarea
              value={customSchemaText}
              onChange={(e) => setCustomSchemaText(e.target.value)}
              rows={2}
              placeholder='{"type": "object", "properties": {}}'
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
              {tx("上一步", "Back")}
            </Button>
            <Button
              type="button"
              data-testid="publish-submit-btn"
              disabled={submitting}
              onClick={() => void handlePublish()}
            >
              {submitting
                ? tx("发布中…", "Publishing…")
                : tx("发布端点", "Publish")}
            </Button>
          </div>
        </div>
      ) : createdEndpoint ? (
        <div className="space-y-4 text-xs" data-testid="publish-step3-done">
          <div className="flex items-center gap-2 rounded-md border border-xblue/40 bg-xblue-soft/40 p-3">
            <StatusBadge status={createdEndpoint.environment} />
            <span className="font-semibold">{createdEndpoint.title}</span>
            <span className="font-mono-tech text-muted-foreground">
              ({createdEndpoint.slug})
            </span>
          </div>

          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <span className="font-medium">cURL</span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void copySnippet(curlSnippet, "curl")}
              >
                {copiedField === "curl" ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制 cURL", "Copy cURL")}
              </Button>
            </div>
            <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto">
              {curlSnippet}
            </pre>
          </div>

          {createdSkill ? (
            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <span className="font-medium">skill.json</span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    void copySnippet(
                      JSON.stringify(createdSkill, null, 2),
                      "skill"
                    )
                  }
                >
                  {copiedField === "skill" ? (
                    <Check className="h-3.5 w-3.5 text-xblue" />
                  ) : (
                    <Copy className="h-3.5 w-3.5" />
                  )}
                  {tx("复制 JSON", "Copy JSON")}
                </Button>
              </div>
              <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-44">
                {JSON.stringify(createdSkill, null, 2)}
              </pre>
            </div>
          ) : null}

          <div className="flex justify-end">
            <Button type="button" onClick={onClose}>
              {tx("关闭", "Close")}
            </Button>
          </div>
        </div>
      ) : null}
    </Modal>
  );
}
