import { useMemo, useState } from "react";
import { z } from "zod";
import {
  Activity,
  CheckCircle2,
  Edit3,
  KeyRound,
  Plus,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import {
  createLlmProvider,
  deleteLlmProvider,
  testLlmProvider,
  updateLlmProvider,
  type LlmProviderRecord,
  type LlmPurpose,
  type LlmUsageRecord,
  type ProviderTestResponse,
  type SaveLlmProviderRequest,
} from "@/lib/api/axagent";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

const ProviderFormSchema = z.object({
  name: z.string().trim().min(1, "提供方名称不能为空").max(64),
  base_url: z.string().trim().url("API 根地址必须是合法 URL"),
  model: z.string().trim().min(1, "默认模型名称不能为空"),
  purposes: z.array(z.string()).min(1, "至少勾选一项模型用途"),
  priority: z.number().int().min(1).max(10000),
  temperature: z.number().min(0).max(2),
  max_tokens: z.number().int().min(64).max(131072),
  timeout_s: z.number().int().min(1).max(600),
});

const ALL_PURPOSES: readonly LlmPurpose[] = [
  "AGENT",
  "CHAT",
  "NL2SQL",
  "SUMMARIZE",
  "EXTRACT",
  "EMBED",
];

export interface ProviderTableProps {
  providers: LlmProviderRecord[];
  usage: LlmUsageRecord[];
  usageFrom: string;
  usageTo: string;
  onChangeUsageRange: (from: string, to: string) => void;
  onRefresh: () => Promise<void>;
}

export function ProviderTable({
  providers,
  usage,
  usageFrom,
  usageTo,
  onChangeUsageRange,
  onRefresh,
}: ProviderTableProps) {
  const sortedProviders = useMemo(
    () => [...providers].sort((a, b) => a.priority - b.priority),
    [providers]
  );

  const [modalOpen, setModalOpen] = useState(false);
  const [editingProvider, setEditingProvider] =
    useState<LlmProviderRecord | null>(null);

  const [presetKind, setPresetKind] = useState<
    "OPENAI_COMPAT" | "VLLM" | "OLLAMA"
  >("OPENAI_COMPAT");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("https://api.openai.com/v1");
  const [model, setModel] = useState("gpt-4o-mini");
  const [apiKey, setApiKey] = useState("");
  const [purposes, setPurposes] = useState<LlmPurpose[]>([
    "AGENT",
    "CHAT",
    "NL2SQL",
  ]);
  const [priority, setPriority] = useState("100");
  const [temperature, setTemperature] = useState("0.2");
  const [maxTokens, setMaxTokens] = useState("4096");
  const [timeoutS, setTimeoutS] = useState("60");
  const [rpmLimit, setRpmLimit] = useState("");
  const [supportsTools, setSupportsTools] = useState(true);
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);
  const [enabled, setEnabled] = useState(true);

  const [saving, setSaving] = useState(false);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, ProviderTestResponse>
  >({});
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const openCreateModal = () => {
    setEditingProvider(null);
    setPresetKind("OPENAI_COMPAT");
    setName("");
    setBaseUrl("https://api.openai.com/v1");
    setModel("gpt-4o-mini");
    setApiKey("");
    setPurposes(["AGENT", "CHAT", "NL2SQL"]);
    setPriority("100");
    setTemperature("0.2");
    setMaxTokens("4096");
    setTimeoutS("60");
    setRpmLimit("");
    setSupportsTools(true);
    setAllowPrivateNetwork(false);
    setEnabled(true);
    setErrorMsg(null);
    setModalOpen(true);
  };

  const openEditModal = (p: LlmProviderRecord) => {
    setEditingProvider(p);
    setName(p.name);
    setBaseUrl(p.base_url);
    setModel(p.model);
    setApiKey("");
    setPurposes(p.purposes);
    setPriority(String(p.priority));
    setTemperature(String(p.temperature));
    setMaxTokens(String(p.max_tokens));
    setTimeoutS(String(p.timeout_s));
    setRpmLimit(p.rpm_limit !== null ? String(p.rpm_limit) : "");
    setSupportsTools(p.supports_tools);
    setAllowPrivateNetwork(p.allow_private_network);
    setEnabled(p.enabled);
    setErrorMsg(null);
    setModalOpen(true);
  };

  const applyPreset = (nextKind: "OPENAI_COMPAT" | "VLLM" | "OLLAMA") => {
    setPresetKind(nextKind);
    if (!editingProvider) {
      if (nextKind === "VLLM") {
        setBaseUrl("http://127.0.0.1:8000/v1");
      } else if (nextKind === "OLLAMA") {
        setBaseUrl("http://127.0.0.1:11434/v1");
      } else {
        setBaseUrl("https://api.openai.com/v1");
      }
    }
  };

  const togglePurpose = (p: LlmPurpose) => {
    setPurposes((prev) =>
      prev.includes(p) ? prev.filter((x) => x !== p) : [...prev, p]
    );
  };

  const handleSave = async () => {
    setErrorMsg(null);
    const check = ProviderFormSchema.safeParse({
      name,
      base_url: baseUrl,
      model,
      purposes,
      priority: Number(priority),
      temperature: Number(temperature),
      max_tokens: Number(maxTokens),
      timeout_s: Number(timeoutS),
    });
    if (!check.success) {
      setErrorMsg(check.error.issues[0]?.message ?? "参数格式校验失败");
      return;
    }

    const req: SaveLlmProviderRequest = {
      name: check.data.name,
      base_url: check.data.base_url,
      model: check.data.model,
      purposes,
      priority: check.data.priority,
      temperature: check.data.temperature,
      max_tokens: check.data.max_tokens,
      timeout_s: check.data.timeout_s,
      rpm_limit: rpmLimit.trim().length > 0 ? Number(rpmLimit) : null,
      supports_tools: supportsTools,
      allow_private_network: allowPrivateNetwork,
      enabled,
      ...(apiKey.trim().length > 0 ? { api_key: apiKey.trim() } : {}),
    };

    setSaving(true);
    try {
      if (editingProvider) {
        await updateLlmProvider(editingProvider.id, req);
      } else {
        await createLlmProvider(req);
      }
      setModalOpen(false);
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "保存模型提供方失败");
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async (p: LlmProviderRecord) => {
    setTestingId(p.id);
    setErrorMsg(null);
    try {
      const res = await testLlmProvider(p.id);
      setTestResults((prev) => ({ ...prev, [p.id]: res }));
    } catch (err) {
      setErrorMsg(
        err instanceof Error ? err.message : `测试提供方 ${p.name} 失败`
      );
    } finally {
      setTestingId(null);
    }
  };

  const handleDelete = async (p: LlmProviderRecord) => {
    setErrorMsg(null);
    try {
      await deleteLlmProvider(p.id);
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "删除提供方失败");
    }
  };

  const usageTotals = useMemo(() => {
    let requests = 0;
    let errors = 0;
    let promptTokens = 0;
    let completionTokens = 0;
    for (const row of usage) {
      requests += row.requests;
      errors += row.errors;
      promptTokens += row.prompt_tokens;
      completionTokens += row.completion_tokens;
    }
    return { requests, errors, promptTokens, completionTokens };
  }, [usage]);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">
            多模型路由注册表 (LlmRouter)
          </h2>
          <p className="text-xs text-muted-foreground">
            同一用途按 priority 升序路由，连续失败自动熔断 60 秒并故障切换至下一提供方
          </p>
        </div>
        <Button
          type="button"
          data-testid="new-provider-btn"
          onClick={openCreateModal}
        >
          <Plus className="h-3.5 w-3.5" />
          注册模型提供方
        </Button>
      </div>

      {errorMsg ? (
        <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
          {errorMsg}
        </div>
      ) : null}

      {/* Providers Table */}
      <Card className="overflow-hidden">
        {sortedProviders.length === 0 ? (
          <div className="p-8 text-center text-xs text-muted-foreground">
            当前租户未单独注册模型提供方，默认回退至平台环境变量网关 (LLM_*)
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-4 py-2.5 font-medium">优先级</th>
                  <th className="px-3 py-2.5 font-medium">提供方 / 模型</th>
                  <th className="px-3 py-2.5 font-medium">支持用途</th>
                  <th className="px-3 py-2.5 font-medium">密钥与能力</th>
                  <th className="px-3 py-2.5 font-medium">连通性测试</th>
                  <th className="px-4 py-2.5 font-medium text-right">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {sortedProviders.map((p) => {
                  const testRes = testResults[p.id];
                  return (
                    <tr
                      key={p.id}
                      data-testid={`provider-row-${p.name}`}
                      className="hover:bg-muted/20"
                    >
                      <td className="px-4 py-3">
                        <Badge variant={p.enabled ? "xblue" : "outline"}>
                          P{p.priority}
                        </Badge>
                      </td>
                      <td className="px-3 py-3 space-y-0.5">
                        <div className="font-semibold flex items-center gap-1.5">
                          <span>{p.name}</span>
                          {!p.enabled ? (
                            <Badge variant="outline">已停用</Badge>
                          ) : null}
                        </div>
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {p.model} · {p.base_url}
                        </div>
                      </td>
                      <td className="px-3 py-3">
                        <div className="flex flex-wrap gap-1">
                          {p.purposes.map((pur) => (
                            <Badge key={pur} variant="neutral">
                              {pur}
                            </Badge>
                          ))}
                        </div>
                      </td>
                      <td className="px-3 py-3 space-y-1">
                        <div className="flex items-center gap-1.5">
                          <Badge variant="outline">
                            <KeyRound className="h-2.5 w-2.5 mr-1" />
                            {p.has_api_key ? "密钥环已封存" : "免密钥"}
                          </Badge>
                          {p.supports_tools ? (
                            <Badge variant="outline">Tools</Badge>
                          ) : null}
                        </div>
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {p.rpm_limit ? `${p.rpm_limit} RPM` : "不限 RPM"} ·{" "}
                          {p.timeout_s}s
                        </div>
                      </td>
                      <td className="px-3 py-3">
                        {testRes ? (
                          <div
                            data-testid={`provider-test-result-${p.name}`}
                            className="space-y-0.5 text-[11px]"
                          >
                            <div className="flex items-center gap-1 font-mono-tech text-xblue">
                              <CheckCircle2 className="h-3.5 w-3.5" />
                              {testRes.latency_ms}ms · {testRes.model}
                            </div>
                            <div className="text-muted-foreground truncate max-w-[180px]">
                              {testRes.reply}
                            </div>
                          </div>
                        ) : (
                          <span className="text-[11px] text-muted-foreground">
                            未测试
                          </span>
                        )}
                      </td>
                      <td className="px-4 py-3">
                        <div className="flex items-center justify-end gap-1.5">
                          <Button
                            type="button"
                            variant="outline"
                            data-testid={`test-provider-btn-${p.name}`}
                            disabled={testingId === p.id}
                            onClick={() => void handleTest(p)}
                            className="h-7 px-2.5"
                          >
                            <Zap className="h-3.5 w-3.5 text-xblue" />
                            {testingId === p.id ? "测试中…" : "测试连接"}
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            onClick={() => openEditModal(p)}
                            className="h-7 px-2"
                          >
                            <Edit3 className="h-3.5 w-3.5" />
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            onClick={() => void handleDelete(p)}
                            className="h-7 px-2"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      {/* Daily LLM Token Usage Summary (`ax_llm_usage`) */}
      <Card className="p-4 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <Activity className="h-4 w-4 text-xblue" />
            <div>
              <h3 className="text-sm font-semibold">
                每日模型调用与 Token 用量统计 (ax_llm_usage)
              </h3>
              <p className="text-[11px] text-muted-foreground">
                按提供方、日期与路由用途聚合请求次数、错误数及输入/输出 Token
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2 text-xs">
            <Input
              type="date"
              value={usageFrom}
              onChange={(e) => onChangeUsageRange(e.target.value, usageTo)}
              className="h-8 w-36 text-xs font-mono-tech"
            />
            <span className="text-muted-foreground">至</span>
            <Input
              type="date"
              value={usageTo}
              onChange={(e) => onChangeUsageRange(usageFrom, e.target.value)}
              className="h-8 w-36 text-xs font-mono-tech"
            />
          </div>
        </div>

        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 text-xs">
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">总请求数</div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {usageTotals.requests.toLocaleString()}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">错误次数</div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {usageTotals.errors.toLocaleString()}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">
              输入 Prompt Tokens
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {usageTotals.promptTokens.toLocaleString()}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">
              输出 Completion Tokens
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {usageTotals.completionTokens.toLocaleString()}
            </div>
          </div>
        </div>

        {usage.length === 0 ? (
          <p className="text-xs text-muted-foreground py-4 text-center border border-dashed border-border rounded-md">
            当前时间范围内暂无模型调用计量记录
          </p>
        ) : (
          <div className="overflow-x-auto rounded-md border border-border">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-3 py-2 font-medium">日期</th>
                  <th className="px-3 py-2 font-medium">提供方</th>
                  <th className="px-3 py-2 font-medium">用途</th>
                  <th className="px-3 py-2 font-medium text-right">请求 / 错误</th>
                  <th className="px-3 py-2 font-medium text-right">Prompt Tokens</th>
                  <th className="px-3 py-2 font-medium text-right">
                    Completion Tokens
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {usage.map((u, idx) => (
                  <tr
                    key={`${u.provider_id}-${u.usage_date}-${u.purpose}-${idx}`}
                  >
                    <td className="px-3 py-2 font-mono-tech">{u.usage_date}</td>
                    <td className="px-3 py-2 font-medium">{u.provider_name}</td>
                    <td className="px-3 py-2">
                      <Badge variant="outline">{u.purpose}</Badge>
                    </td>
                    <td className="px-3 py-2 text-right font-mono-tech">
                      {u.requests} / {u.errors}
                    </td>
                    <td className="px-3 py-2 text-right font-mono-tech">
                      {u.prompt_tokens.toLocaleString()}
                    </td>
                    <td className="px-3 py-2 text-right font-mono-tech">
                      {u.completion_tokens.toLocaleString()}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      {/* Create / Edit Provider Modal */}
      {modalOpen ? (
        <div
          data-testid="provider-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        >
          <Card className="w-full max-w-xl max-h-[90vh] overflow-y-auto p-5 space-y-4 shadow-lg text-xs">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <h3 className="text-sm font-semibold">
                {editingProvider
                  ? `编辑模型提供方 · ${editingProvider.name}`
                  : "注册新模型提供方"}
              </h3>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setModalOpen(false)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {!editingProvider ? (
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-muted-foreground">
                  接口协议预设:
                </span>
                {(
                  [
                    ["OPENAI_COMPAT", "OpenAI 兼容网关"],
                    ["VLLM", "本地 vLLM"],
                    ["OLLAMA", "Ollama /v1"],
                  ] as const
                ).map(([k, label]) => (
                  <button
                    key={k}
                    type="button"
                    onClick={() => applyPreset(k)}
                    className={cn(
                      "rounded border px-2.5 py-1 text-[11px] cursor-pointer",
                      presetKind === k
                        ? "border-xblue bg-xblue-soft text-xblue"
                        : "border-border text-muted-foreground"
                    )}
                  >
                    {label}
                  </button>
                ))}
              </div>
            ) : null}

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  唯一名称 (name)
                </label>
                <Input
                  data-testid="provider-name-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="primary-openai"
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  默认模型名 (model)
                </label>
                <Input
                  data-testid="provider-model-input"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  placeholder="gpt-4o-mini"
                  className="font-mono-tech text-xs"
                />
              </div>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  OpenAI 兼容地址 (base_url)
                </label>
                <Input
                  data-testid="provider-baseurl-input"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  API 密钥（加密存入 ds_credentials，不回显）
                </label>
                <Input
                  type="password"
                  data-testid="provider-apikey-input"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder={
                    editingProvider?.has_api_key
                      ? "留空保持原封存密钥不变"
                      : "sk-..."
                  }
                  className="font-mono-tech text-xs"
                />
              </div>
            </div>

            <div className="space-y-1.5">
              <label className="text-[11px] text-muted-foreground">
                路由用途 (purposes)
              </label>
              <div className="flex flex-wrap gap-2">
                {ALL_PURPOSES.map((p) => (
                  <label
                    key={p}
                    className="inline-flex items-center gap-1.5 rounded border border-border px-2.5 py-1 font-mono-tech text-[11px] cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={purposes.includes(p)}
                      onChange={() => togglePurpose(p)}
                      className="accent-[#2563eb]"
                    />
                    {p}
                  </label>
                ))}
              </div>
            </div>

            <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  优先级 (小者优先)
                </label>
                <Input
                  type="number"
                  value={priority}
                  onChange={(e) => setPriority(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  限流 RPM (可选)
                </label>
                <Input
                  type="number"
                  value={rpmLimit}
                  onChange={(e) => setRpmLimit(e.target.value)}
                  placeholder="不限"
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  超时秒数 (timeout_s)
                </label>
                <Input
                  type="number"
                  value={timeoutS}
                  onChange={(e) => setTimeoutS(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">
                  最大输出 Token
                </label>
                <Input
                  type="number"
                  value={maxTokens}
                  onChange={(e) => setMaxTokens(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
            </div>

            <div className="flex flex-wrap gap-4 pt-1">
              <label className="inline-flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={supportsTools}
                  onChange={(e) => setSupportsTools(e.target.checked)}
                  className="accent-[#2563eb]"
                />
                <span>支持 Tool / Function Calling</span>
              </label>
              <label className="inline-flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={allowPrivateNetwork}
                  onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
                  className="accent-[#2563eb]"
                />
                <span>允许私网地址 (需 GUARD_ADMIN)</span>
              </label>
              <label className="inline-flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={enabled}
                  onChange={(e) => setEnabled(e.target.checked)}
                  className="accent-[#2563eb]"
                />
                <span>启用该提供方</span>
              </label>
            </div>

            <div className="flex justify-end gap-2 pt-2 border-t border-border">
              <Button
                type="button"
                variant="outline"
                onClick={() => setModalOpen(false)}
              >
                取消
              </Button>
              <Button
                type="button"
                data-testid="save-provider-btn"
                disabled={saving}
                onClick={() => void handleSave()}
              >
                {saving ? "保存中…" : "保存提供方"}
              </Button>
            </div>
          </Card>
        </div>
      ) : null}
    </div>
  );
}
