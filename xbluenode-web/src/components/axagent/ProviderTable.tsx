import { useCallback, useMemo, useState } from "react";
import { z } from "zod";
import {
  Activity,
  CheckCircle2,
  Edit3,
  KeyRound,
  Plus,
  Trash2,
  XCircle,
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
  type ProviderScope,
  type ProviderTestResponse,
  type SaveLlmProviderRequest,
  type StructuredOutputMode,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Card,
  Input,
  Modal,
  PasswordInput,
  Select,
  StatusBadge,
  Switch,
  type SelectOptionItem,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";

const ALL_PURPOSES: readonly LlmPurpose[] = [
  "AGENT",
  "CHAT",
  "NL2SQL",
  "SUMMARIZE",
  "EXTRACT",
  "EMBED",
];

const STRUCTURED_MODES: readonly StructuredOutputMode[] = [
  "NONE",
  "JSON_OBJECT",
  "JSON_SCHEMA",
];

function isStructuredOutputMode(val: string): val is StructuredOutputMode {
  return (
    val === "NONE" || val === "JSON_OBJECT" || val === "JSON_SCHEMA"
  );
}

function isProviderScope(val: string): val is ProviderScope {
  return val === "SYSTEM" || val === "PERSONAL";
}

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
  const { tx, tEnum, fmtNum } = useI18n();
  const { me, can, canAny } = usePrincipal();
  const canGuardAdmin = can("GUARD_ADMIN");
  const canSystemAdmin = canAny(["GUARD_ADMIN", "OPS_ADMIN"]);
  const canTestSystem = canAny(["AGENT_EDIT", "GUARD_ADMIN", "OPS_ADMIN"]);
  const canCreateProvider = canAny([
    "GUARD_ADMIN",
    "OPS_ADMIN",
    "AGENT_EDIT",
    "AGENT_RUN",
    "DATA_READ",
  ]);

  const isProviderReadonly = useCallback(
    (p: LlmProviderRecord): boolean => {
      const isSystem = !p.owner_user_id;
      return p.is_readonly !== undefined
        ? p.is_readonly
        : isSystem && !canSystemAdmin;
    },
    [canSystemAdmin]
  );

  const canEditProvider = useCallback(
    (p: LlmProviderRecord): boolean => {
      if (isProviderReadonly(p)) return false;
      if (!p.owner_user_id) {
        return canSystemAdmin;
      }
      return (
        (!me?.user_id || p.owner_user_id === me.user_id) && canCreateProvider
      );
    },
    [canCreateProvider, canSystemAdmin, isProviderReadonly, me?.user_id]
  );

  const canTestRow = useCallback(
    (p: LlmProviderRecord): boolean => {
      if (!p.owner_user_id) {
        return canTestSystem;
      }
      return !me?.user_id || p.owner_user_id === me.user_id;
    },
    [canTestSystem, me?.user_id]
  );

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
  const [scope, setScope] = useState<ProviderScope>("SYSTEM");
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
  const [maxContextTokens, setMaxContextTokens] = useState("32768");
  const [timeoutS, setTimeoutS] = useState("60");
  const [rpmLimit, setRpmLimit] = useState("");
  const [structuredMode, setStructuredMode] =
    useState<StructuredOutputMode>("NONE");
  const [deterministicSeed, setDeterministicSeed] = useState("");
  const [isLocalModel, setIsLocalModel] = useState(false);
  const [supportsTools, setSupportsTools] = useState(true);
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);
  const [enabled, setEnabled] = useState(true);

  const [saving, setSaving] = useState(false);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, ProviderTestResponse>
  >({});
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const effectiveScope: ProviderScope = editingProvider
    ? editingProvider.owner_user_id
      ? "PERSONAL"
      : "SYSTEM"
    : canSystemAdmin
      ? scope
      : "PERSONAL";

  const scopeOptions = useMemo<readonly SelectOptionItem[]>(
    () => [
      {
        value: "SYSTEM",
        label: tx("系统", "System"),
        text: tx("系统", "System"),
        disabled: !canSystemAdmin,
      },
      {
        value: "PERSONAL",
        label: tx("个人", "Personal"),
        text: tx("个人", "Personal"),
      },
    ],
    [canSystemAdmin, tx]
  );

  const structuredModeOptions = useMemo<readonly SelectOptionItem[]>(
    () =>
      STRUCTURED_MODES.map((m) => ({
        value: m,
        label: m,
        text: m,
      })),
    []
  );

  const openCreateModal = () => {
    setEditingProvider(null);
    setPresetKind("OPENAI_COMPAT");
    setScope(canSystemAdmin ? "SYSTEM" : "PERSONAL");
    setName("");
    setBaseUrl("https://api.openai.com/v1");
    setModel("gpt-4o-mini");
    setApiKey("");
    setPurposes(["AGENT", "CHAT", "NL2SQL"]);
    setPriority("100");
    setTemperature("0.2");
    setMaxTokens("4096");
    setMaxContextTokens("32768");
    setTimeoutS("60");
    setRpmLimit("");
    setStructuredMode("NONE");
    setDeterministicSeed("");
    setIsLocalModel(false);
    setSupportsTools(true);
    setAllowPrivateNetwork(false);
    setEnabled(true);
    setErrorMsg(null);
    setModalOpen(true);
  };

  const openEditModal = (p: LlmProviderRecord) => {
    if (!canEditProvider(p)) return;
    setEditingProvider(p);
    setScope(p.owner_user_id ? "PERSONAL" : "SYSTEM");
    setName(p.name);
    setBaseUrl(p.base_url);
    setModel(p.model);
    setApiKey("");
    setPurposes(p.purposes);
    setPriority(String(p.priority));
    setTemperature(String(p.temperature));
    setMaxTokens(String(p.max_tokens));
    setMaxContextTokens(String(p.max_context_tokens ?? 32768));
    setTimeoutS(String(p.timeout_s));
    setRpmLimit(p.rpm_limit !== null ? String(p.rpm_limit) : "");
    setStructuredMode(p.structured_mode ?? "NONE");
    setDeterministicSeed(
      p.deterministic_seed !== null && p.deterministic_seed !== undefined
        ? String(p.deterministic_seed)
        : ""
    );
    setIsLocalModel(Boolean(p.is_local_model));
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
        setStructuredMode("JSON_SCHEMA");
        setIsLocalModel(true);
        setDeterministicSeed("42");
        if (canGuardAdmin) {
          setAllowPrivateNetwork(true);
        }
      } else if (nextKind === "OLLAMA") {
        setBaseUrl("http://127.0.0.1:11434/v1");
        setStructuredMode("JSON_SCHEMA");
        setIsLocalModel(true);
        setDeterministicSeed("42");
        if (canGuardAdmin) {
          setAllowPrivateNetwork(true);
        }
      } else {
        setBaseUrl("https://api.openai.com/v1");
        setStructuredMode("NONE");
        setIsLocalModel(false);
        setDeterministicSeed("");
        setAllowPrivateNetwork(false);
      }
    }
  };

  const togglePurpose = (p: LlmPurpose) => {
    setPurposes((prev) =>
      prev.includes(p) ? prev.filter((x) => x !== p) : [...prev, p]
    );
  };

  const handleSave = async () => {
    if (editingProvider !== null) {
      if (!canEditProvider(editingProvider)) return;
    } else if (!canCreateProvider) {
      return;
    }

    setErrorMsg(null);
    const providerFormSchema = z.object({
      name: z
        .string()
        .trim()
        .min(1, tx("提供方名称不能为空", "Provider name is required"))
        .max(64),
      base_url: z
        .string()
        .trim()
        .url(tx("接口地址须为合法 URL", "Base URL must be valid")),
      model: z
        .string()
        .trim()
        .min(1, tx("模型名称不能为空", "Model name is required")),
      purposes: z
        .array(z.string())
        .min(1, tx("至少勾选一项用途", "At least one purpose is required")),
      priority: z.number().int().min(1).max(10000),
      temperature: z.number().min(0).max(2),
      max_tokens: z.number().int().min(64).max(131072),
      max_context_tokens: z.number().int().min(2048).max(2097152),
      timeout_s: z.number().int().min(1).max(600),
      deterministic_seed: z.number().int().nullable(),
    });

    const trimmedContext = maxContextTokens.trim();
    const parsedMaxContext =
      trimmedContext.length > 0 ? Number(trimmedContext) : 32768;
    const trimmedSeed = deterministicSeed.trim();
    const parsedSeed = trimmedSeed.length > 0 ? Number(trimmedSeed) : null;

    const check = providerFormSchema.safeParse({
      name,
      base_url: baseUrl,
      model,
      purposes,
      priority: Number(priority),
      temperature: Number(temperature),
      max_tokens: Number(maxTokens),
      max_context_tokens: parsedMaxContext,
      timeout_s: Number(timeoutS),
      deterministic_seed: parsedSeed,
    });
    if (!check.success) {
      setErrorMsg(
        check.error.issues[0]?.message ??
          tx("字段格式有误", "Invalid form input")
      );
      return;
    }

    const effectiveAllowPrivateNetwork = canGuardAdmin
      ? allowPrivateNetwork
      : Boolean(editingProvider?.allow_private_network);

    const req: SaveLlmProviderRequest = {
      name: check.data.name,
      ...(!editingProvider ? { scope: effectiveScope } : {}),
      base_url: check.data.base_url,
      model: check.data.model,
      purposes,
      priority: check.data.priority,
      temperature: check.data.temperature,
      max_tokens: check.data.max_tokens,
      max_context_tokens: check.data.max_context_tokens,
      timeout_s: check.data.timeout_s,
      rpm_limit: rpmLimit.trim().length > 0 ? Number(rpmLimit) : null,
      structured_mode: structuredMode,
      deterministic_seed: check.data.deterministic_seed,
      is_local_model: isLocalModel,
      supports_tools: supportsTools,
      allow_private_network: effectiveAllowPrivateNetwork,
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("保存失败", "Failed to save provider")
      );
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async (p: LlmProviderRecord) => {
    if (!canTestRow(p)) return;
    setTestingId(p.id);
    setErrorMsg(null);
    try {
      const res = await testLlmProvider(p.id);
      setTestResults((prev) => ({ ...prev, [p.id]: res }));
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx(`测试 ${p.name} 失败`, `Failed to test ${p.name}`)
      );
    } finally {
      setTestingId(null);
    }
  };

  const handleDelete = async (p: LlmProviderRecord) => {
    if (!canEditProvider(p)) return;
    setErrorMsg(null);
    try {
      await deleteLlmProvider(p.id);
      await onRefresh();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("删除失败", "Failed to delete provider")
      );
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
        <h2 className="text-sm font-semibold">
          {tx("模型路由注册表", "LLM Provider Router")}
        </h2>
        {canCreateProvider && (
          <Button
            type="button"
            data-testid="new-provider-btn"
            onClick={openCreateModal}
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("注册模型", "New Provider")}
          </Button>
        )}
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
            {tx("暂无模型提供方", "No providers")}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-4 py-2.5 font-medium whitespace-nowrap">
                    {tx("优先级", "Priority")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("提供方 / 模型", "Provider / Model")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("作用域", "Scope")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("解码能力", "Decoding")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("路由用途", "Purposes")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("密钥与能力", "Capabilities")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("连通性", "Connectivity")}
                  </th>
                  <th className="px-4 py-2.5 font-medium text-right whitespace-nowrap">
                    {tx("操作", "Actions")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {sortedProviders.map((p) => {
                  const testRes = testResults[p.id];
                  const isSystem = !p.owner_user_id;
                  const isReadonly = isProviderReadonly(p);
                  const canModifyRow = canEditProvider(p);
                  const allowTestRow = canTestRow(p);
                  const rowStructuredMode: StructuredOutputMode =
                    p.structured_mode ?? "NONE";

                  return (
                    <tr
                      key={p.id}
                      data-testid={`provider-row-${p.name}`}
                      className="hover:bg-muted/20"
                    >
                      <td className="px-4 py-3 whitespace-nowrap">
                        <Badge variant={p.enabled ? "xblue" : "outline"}>
                          P{p.priority}
                        </Badge>
                      </td>
                      <td className="px-3 py-3 space-y-0.5">
                        <div className="font-semibold flex items-center gap-1.5">
                          <span>{p.name}</span>
                          {!p.enabled ? (
                            <StatusBadge status="DISABLED" />
                          ) : null}
                        </div>
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {p.model} · {p.base_url}
                        </div>
                      </td>
                      <td className="px-3 py-3 whitespace-nowrap">
                        <div className="flex items-center flex-wrap gap-1">
                          {isSystem ? (
                            <Badge
                              variant="xblue"
                              data-testid={`provider-scope-system-${p.id}`}
                            >
                              {tx("系统", "System")}
                            </Badge>
                          ) : (
                            <Badge
                              variant="neutral"
                              data-testid={`provider-scope-personal-${p.id}`}
                            >
                              {tx("个人", "Personal")}
                            </Badge>
                          )}
                          {isSystem && isReadonly ? (
                            <Badge
                              variant="outline"
                              data-testid={`provider-readonly-lock-${p.id}`}
                            >
                              {tx("只读", "Read-only")}
                            </Badge>
                          ) : null}
                        </div>
                      </td>
                      <td className="px-3 py-3 whitespace-nowrap">
                        <div className="flex items-center flex-wrap gap-1">
                          <Badge
                            variant={
                              rowStructuredMode === "JSON_SCHEMA"
                                ? "success"
                                : rowStructuredMode === "JSON_OBJECT"
                                  ? "xblue"
                                  : "outline"
                            }
                            data-testid={`provider-structured-badge-${p.id}`}
                          >
                            {rowStructuredMode}
                          </Badge>
                          {p.is_local_model ? (
                            <Badge variant="neutral">
                              {tx("本地", "Local")}
                            </Badge>
                          ) : null}
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
                      <td className="px-3 py-3 space-y-1 whitespace-nowrap">
                        <div className="flex items-center gap-1.5">
                          <Badge variant="outline">
                            <KeyRound className="h-2.5 w-2.5 mr-1" />
                            {p.has_api_key
                              ? tx("已封存", "Sealed")
                              : tx("免密钥", "Keyless")}
                          </Badge>
                          {p.supports_tools ? (
                            <Badge variant="outline">Tools</Badge>
                          ) : null}
                        </div>
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {p.rpm_limit
                            ? `${p.rpm_limit} RPM`
                            : tx("不限 RPM", "Unlimited RPM")}{" "}
                          · {p.timeout_s}s
                        </div>
                      </td>
                      <td className="px-3 py-3 whitespace-nowrap">
                        {testRes ? (
                          <div
                            data-testid={`provider-test-result-${p.name}`}
                            className="space-y-0.5 text-[11px]"
                          >
                            <div
                              className={cn(
                                "flex items-center flex-wrap gap-1.5 font-mono-tech",
                                testRes.ok
                                  ? "text-xblue"
                                  : "text-red-600 dark:text-red-400"
                              )}
                            >
                              {testRes.ok ? (
                                <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
                              ) : (
                                <XCircle className="h-3.5 w-3.5 shrink-0" />
                              )}
                              <span>
                                {testRes.latency_ms}ms · {testRes.model}
                              </span>
                              {!testRes.ok ? (
                                <Badge variant="danger">
                                  {tx("失败", "Failed")}
                                </Badge>
                              ) : null}
                              {testRes.structured_mode &&
                              testRes.structured_mode !== "NONE" ? (
                                <Badge variant="outline">
                                  {tEnum(testRes.structured_mode)}
                                </Badge>
                              ) : null}
                              {testRes.structured_verified ? (
                                <Badge
                                  variant="success"
                                  data-testid={`provider-test-verified-${p.name}`}
                                >
                                  {tx("已核验", "Verified")}
                                </Badge>
                              ) : null}
                            </div>
                            <div className="text-muted-foreground truncate max-w-[180px]">
                              {testRes.reply}
                            </div>
                          </div>
                        ) : (
                          <span className="text-[11px] text-muted-foreground">
                            {tx("未测试", "Untested")}
                          </span>
                        )}
                      </td>
                      <td className="px-4 py-3 whitespace-nowrap">
                        <div className="flex items-center justify-end gap-1.5">
                          {allowTestRow && (
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              data-testid={`test-provider-btn-${p.name}`}
                              disabled={testingId === p.id}
                              onClick={() => void handleTest(p)}
                            >
                              <Zap className="h-3.5 w-3.5 text-xblue" />
                              {testingId === p.id
                                ? tx("测试中…", "Testing…")
                                : tx("测试连接", "Test")}
                            </Button>
                          )}
                          {canModifyRow && (
                            <>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                data-testid={`edit-provider-btn-${p.name}`}
                                aria-label={tx("编辑", "Edit")}
                                onClick={() => openEditModal(p)}
                              >
                                <Edit3 className="h-3.5 w-3.5" />
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                size="sm"
                                data-testid={`delete-provider-btn-${p.name}`}
                                aria-label={tx("删除", "Delete")}
                                onClick={() => void handleDelete(p)}
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </Button>
                            </>
                          )}
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

      {/* Daily LLM Token Usage Summary */}
      <Card className="p-4 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <Activity className="h-4 w-4 text-xblue" />
            <h3 className="text-sm font-semibold">
              {tx("每日模型用量统计", "Daily LLM Token Usage")}
            </h3>
          </div>

          <div className="flex items-center gap-2 text-xs">
            <Input
              type="date"
              value={usageFrom}
              onChange={(e) => onChangeUsageRange(e.target.value, usageTo)}
              className="h-8 w-36 text-xs font-mono-tech"
            />
            <span className="text-muted-foreground">→</span>
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
            <div className="text-[11px] text-muted-foreground">
              {tx("总请求数", "Total Requests")}
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {fmtNum(usageTotals.requests, 0)}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">
              {tx("错误次数", "Errors")}
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {fmtNum(usageTotals.errors, 0)}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">
              Prompt Tokens
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {fmtNum(usageTotals.promptTokens, 0)}
            </div>
          </div>
          <div className="rounded-md border border-border bg-muted/30 p-3">
            <div className="text-[11px] text-muted-foreground">
              Completion Tokens
            </div>
            <div className="text-base font-semibold font-mono-tech mt-0.5">
              {fmtNum(usageTotals.completionTokens, 0)}
            </div>
          </div>
        </div>

        {usage.length === 0 ? (
          <p className="text-xs text-muted-foreground py-4 text-center border border-dashed border-border rounded-md">
            {tx("暂无用量记录", "No usage records")}
          </p>
        ) : (
          <div className="overflow-x-auto rounded-md border border-border">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-3 py-2 font-medium">
                    {tx("日期", "Date")}
                  </th>
                  <th className="px-3 py-2 font-medium">
                    {tx("提供方", "Provider")}
                  </th>
                  <th className="px-3 py-2 font-medium">
                    {tx("用途", "Purpose")}
                  </th>
                  <th className="px-3 py-2 font-medium text-right">
                    {tx("请求 / 错误", "Requests / Errors")}
                  </th>
                  <th className="px-3 py-2 font-medium text-right">
                    Prompt Tokens
                  </th>
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
                      {fmtNum(u.requests, 0)} / {fmtNum(u.errors, 0)}
                    </td>
                    <td className="px-3 py-2 text-right font-mono-tech">
                      {fmtNum(u.prompt_tokens, 0)}
                    </td>
                    <td className="px-3 py-2 text-right font-mono-tech">
                      {fmtNum(u.completion_tokens, 0)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      {/* Create / Edit Provider Modal */}
      <Modal
        open={modalOpen}
        onClose={() => setModalOpen(false)}
        maxWidthClass="max-w-xl"
        testId="provider-modal"
        title={
          editingProvider
            ? tx(
                `编辑模型 · ${editingProvider.name}`,
                `Edit Provider · ${editingProvider.name}`
              )
            : tx("注册模型", "New Provider")
        }
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => setModalOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              data-testid="save-provider-btn"
              disabled={saving}
              onClick={() => void handleSave()}
            >
              {saving
                ? tx("保存中…", "Saving…")
                : tx("保存", "Save")}
            </Button>
          </>
        }
      >
        {!editingProvider ? (
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-[11px] text-muted-foreground">
              {tx("协议预设", "Preset")}:
            </span>
            {(
              [
                ["OPENAI_COMPAT", tx("OpenAI 兼容", "OpenAI Compatible")],
                ["VLLM", "vLLM"],
                ["OLLAMA", "Ollama"],
              ] as const
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                data-testid={`provider-preset-${k}`}
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

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("作用域", "Scope")}
            </label>
            <Select
              className="w-full"
              aria-label={tx("作用域", "Scope")}
              data-testid="provider-scope-select"
              value={effectiveScope}
              options={scopeOptions}
              disabled={Boolean(editingProvider) || !canSystemAdmin}
              onChange={(e) => {
                if (isProviderScope(e.target.value)) {
                  setScope(e.target.value);
                }
              }}
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("提供方名称", "Provider Name")}
            </label>
            <Input
              data-testid="provider-name-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="primary-gateway"
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("默认模型", "Model")}
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
            <label className="block text-[11px] text-muted-foreground">
              {tx("接口根地址", "Base URL")}
            </label>
            <Input
              data-testid="provider-baseurl-input"
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="http://127.0.0.1:8000/v1"
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("API 密钥", "API Key")}
            </label>
            <PasswordInput
              data-testid="provider-apikey-input"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={
                editingProvider?.has_api_key ? "••••••••" : "sk-..."
              }
              className="font-mono-tech text-xs"
            />
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("结构化解码", "Decoding")}
            </label>
            <Select
              className="w-full font-mono-tech"
              aria-label={tx("结构化解码", "Decoding")}
              data-testid="provider-structured-mode-select"
              value={structuredMode}
              options={structuredModeOptions}
              onChange={(e) => {
                if (isStructuredOutputMode(e.target.value)) {
                  setStructuredMode(e.target.value);
                }
              }}
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("随机种子", "Seed")}
            </label>
            <Input
              type="number"
              data-testid="provider-seed-input"
              value={deterministicSeed}
              onChange={(e) => setDeterministicSeed(e.target.value)}
              placeholder="42"
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("上下文上限", "Max Context")}
            </label>
            <Input
              type="number"
              data-testid="provider-max-context-input"
              value={maxContextTokens}
              onChange={(e) => setMaxContextTokens(e.target.value)}
              placeholder="32768"
              className="font-mono-tech text-xs"
            />
          </div>
        </div>

        <div className="space-y-1.5">
          <label className="block text-[11px] text-muted-foreground">
            {tx("路由用途", "Purposes")}
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
                  className="accent-xblue"
                />
                {p}
              </label>
            ))}
          </div>
        </div>

        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("优先级", "Priority")}
            </label>
            <Input
              type="number"
              value={priority}
              onChange={(e) => setPriority(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("限流 (RPM)", "RPM Limit")}
            </label>
            <Input
              type="number"
              value={rpmLimit}
              onChange={(e) => setRpmLimit(e.target.value)}
              placeholder="120"
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("超时（秒）", "Timeout (s)")}
            </label>
            <Input
              type="number"
              value={timeoutS}
              onChange={(e) => setTimeoutS(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="block text-[11px] text-muted-foreground">
              {tx("最大 Token", "Max Tokens")}
            </label>
            <Input
              type="number"
              value={maxTokens}
              onChange={(e) => setMaxTokens(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-4 pt-1">
          <div className="inline-flex items-center gap-2">
            <Switch
              checked={isLocalModel}
              onCheckedChange={setIsLocalModel}
              aria-label={tx("本地小模型", "Local Model")}
              data-testid="provider-local-model-switch"
            />
            <span>{tx("本地小模型", "Local Model")}</span>
          </div>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={supportsTools}
              onChange={(e) => setSupportsTools(e.target.checked)}
              className="accent-xblue"
            />
            <span>{tx("支持工具调用", "Supports Tools")}</span>
          </label>
          <label
            className={cn(
              "inline-flex items-center gap-2",
              canGuardAdmin ? "cursor-pointer" : "opacity-50"
            )}
          >
            <input
              type="checkbox"
              checked={allowPrivateNetwork}
              disabled={!canGuardAdmin}
              onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
              className="accent-xblue"
            />
            <span>{tx("允许私网地址", "Allow Private Network")}</span>
          </label>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
              className="accent-xblue"
            />
            <span>{tx("启用提供方", "Enable Provider")}</span>
          </label>
        </div>
      </Modal>
    </div>
  );
}
