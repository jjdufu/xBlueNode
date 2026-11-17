import { useCallback, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import {
  Blocks,
  Bot,
  Check,
  Copy,
  Cpu,
  Globe,
  KeyRound,
  LayoutGrid,
  MessageSquare,
  Plug,
  Trash2,
  X,
} from "lucide-react";
import { analyticsApi } from "@/lib/api/analytics";
import { fetchMe } from "@/lib/api/auth";
import type { Permission } from "@/lib/api/enums";
import { xblueApi } from "@/lib/api/xblue";
import {
  createApiToken,
  fetchLlmUsage,
  listAgents,
  listApiTokens,
  listEndpoints,
  listLlmProviders,
  listMcpClients,
  revokeApiToken,
  type CreateApiTokenResponse,
} from "@/lib/api/axagent";
import { AgentEditor } from "@/components/axagent/AgentEditor";
import { AppCard } from "@/components/axagent/AppCard";
import { ChatWorkbench } from "@/components/axagent/ChatWorkbench";
import { EndpointList } from "@/components/axagent/EndpointList";
import { McpClientForm } from "@/components/axagent/McpClientForm";
import { ProviderTable } from "@/components/axagent/ProviderTable";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

type AxAgentTabKey =
  | "workbench"
  | "agents"
  | "apps"
  | "endpoints"
  | "providers"
  | "mcp";

const TABS: ReadonlyArray<{
  key: AxAgentTabKey;
  label: string;
  icon: typeof Bot;
}> = [
  { key: "workbench", label: "工作台", icon: MessageSquare },
  { key: "agents", label: "智能体", icon: Bot },
  { key: "apps", label: "应用", icon: LayoutGrid },
  { key: "endpoints", label: "发布", icon: Globe },
  { key: "providers", label: "模型", icon: Cpu },
  { key: "mcp", label: "MCP", icon: Plug },
];

const DEFAULT_PAT_SCOPES: readonly Permission[] = [
  "DATA_READ",
  "AGENT_RUN",
  "FLOW_RUN",
  "SIM_RUN",
];

const CreatePatFormSchema = z.object({
  name: z.string().trim().min(1, "令牌名称不能为空").max(64),
  scopes: z.array(z.string()).min(1, "至少勾选一项令牌权限"),
  expires_in_days: z.number().int().min(1).max(365),
});

export function AxAgentPage() {
  const queryClient = useQueryClient();

  const [activeTab, setActiveTab] = useState<AxAgentTabKey>("workbench");
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);

  // Active testing PAT stored only in memory (never written to localStorage)
  const [activePat, setActivePat] = useState<string | null>(null);
  const [patModalOpen, setPatModalOpen] = useState(false);
  const [patName, setPatName] = useState("workbench-test-pat");
  const [patDays, setPatDays] = useState("30");
  const [patScopes, setPatScopes] = useState<Permission[]>([
    "DATA_READ",
    "AGENT_RUN",
  ]);
  const [creatingPat, setCreatingPat] = useState(false);
  const [issuedPat, setIssuedPat] = useState<CreateApiTokenResponse | null>(
    null
  );
  const [copiedPat, setCopiedPat] = useState(false);
  const [patError, setPatError] = useState<string | null>(null);

  // Date range for daily LLM usage table
  const [usageFrom, setUsageFrom] = useState("");
  const [usageTo, setUsageTo] = useState("");

  const meQuery = useQuery({
    queryKey: ["axagent", "me"],
    queryFn: async () => {
      try {
        return await fetchMe();
      } catch {
        return null;
      }
    },
  });

  const agentsQuery = useQuery({
    queryKey: ["axagent", "agents"],
    queryFn: () => listAgents(),
  });

  const endpointsQuery = useQuery({
    queryKey: ["axagent", "endpoints"],
    queryFn: () => listEndpoints(),
  });

  const appEndpointsQuery = useQuery({
    queryKey: ["axagent", "endpoints", "apps"],
    queryFn: () => listEndpoints({ show_as_app: true }),
    enabled: activeTab === "apps",
  });

  const providersQuery = useQuery({
    queryKey: ["axagent", "providers"],
    queryFn: async () => {
      try {
        return await listLlmProviders();
      } catch {
        return [];
      }
    },
  });

  const mcpClientsQuery = useQuery({
    queryKey: ["axagent", "mcp-clients"],
    queryFn: async () => {
      try {
        return await listMcpClients();
      } catch {
        return [];
      }
    },
  });

  const usageQuery = useQuery({
    queryKey: ["axagent", "usage", usageFrom, usageTo],
    queryFn: async () => {
      try {
        return await fetchLlmUsage({
          ...(usageFrom ? { from: usageFrom } : {}),
          ...(usageTo ? { to: usageTo } : {}),
        });
      } catch {
        return [];
      }
    },
    enabled: activeTab === "providers",
  });

  const patListQuery = useQuery({
    queryKey: ["axagent", "pat-list"],
    queryFn: () => listApiTokens(),
    enabled: patModalOpen,
  });

  const metricNamesQuery = useQuery({
    queryKey: ["axagent", "ref-metrics"],
    queryFn: async () => {
      try {
        const list = await analyticsApi.listMetrics();
        return Array.isArray(list) ? list.map((m) => m.name) : [];
      } catch {
        return [];
      }
    },
    enabled: activeTab === "endpoints",
  });

  const objectTypeNamesQuery = useQuery({
    queryKey: ["axagent", "ref-object-types"],
    queryFn: async () => {
      try {
        const schema = await xblueApi.getSchema();
        return Array.isArray(schema?.node_types)
          ? schema.node_types.map((nt) => nt.name)
          : [];
      } catch {
        return [];
      }
    },
    enabled: activeTab === "endpoints",
  });

  const agents = useMemo(() => agentsQuery.data ?? [], [agentsQuery.data]);
  const endpoints = useMemo(
    () => endpointsQuery.data ?? [],
    [endpointsQuery.data]
  );
  const appEndpoints = useMemo(() => {
    if (appEndpointsQuery.data) {
      return appEndpointsQuery.data;
    }
    return endpoints.filter((e) => e.show_as_app);
  }, [appEndpointsQuery.data, endpoints]);
  const providers = useMemo(
    () => providersQuery.data ?? [],
    [providersQuery.data]
  );
  const mcpClients = useMemo(
    () => mcpClientsQuery.data ?? [],
    [mcpClientsQuery.data]
  );

  const userPermissions = useMemo<Permission[]>(
    () => meQuery.data?.permissions ?? [...DEFAULT_PAT_SCOPES],
    [meQuery.data]
  );

  const prodEndpointsCount = useMemo(
    () => endpoints.filter((e) => e.environment === "PROD").length,
    [endpoints]
  );
  const queryAppsCount = useMemo(
    () => endpoints.filter((e) => e.show_as_app).length,
    [endpoints]
  );

  const refreshAll = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["axagent", "agents"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "endpoints"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "providers"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "mcp-clients"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "usage"] }),
    ]);
  }, [queryClient]);

  const togglePatScope = (perm: Permission) => {
    setPatScopes((prev) =>
      prev.includes(perm) ? prev.filter((p) => p !== perm) : [...prev, perm]
    );
  };

  const handleIssuePat = async () => {
    setPatError(null);
    const check = CreatePatFormSchema.safeParse({
      name: patName,
      scopes: patScopes,
      expires_in_days: Number(patDays),
    });
    if (!check.success) {
      setPatError(check.error.issues[0]?.message ?? "参数校验失败");
      return;
    }

    setCreatingPat(true);
    try {
      const created = await createApiToken({
        name: check.data.name,
        scopes: patScopes,
        expires_in_days: check.data.expires_in_days,
      });
      setIssuedPat(created);
      setActivePat(created.token);
      await queryClient.invalidateQueries({ queryKey: ["axagent", "pat-list"] });
    } catch (err) {
      setPatError(err instanceof Error ? err.message : "签发个人访问令牌失败");
    } finally {
      setCreatingPat(false);
    }
  };

  const handleRevokePat = async (id: string) => {
    setPatError(null);
    try {
      await revokeApiToken(id);
      if (issuedPat?.id === id) {
        setIssuedPat(null);
        setActivePat(null);
      }
      await queryClient.invalidateQueries({ queryKey: ["axagent", "pat-list"] });
    } catch (err) {
      setPatError(err instanceof Error ? err.message : "吊销访问令牌失败");
    }
  };

  const handleCopyPat = async (token: string) => {
    try {
      await navigator.clipboard.writeText(token);
      setCopiedPat(true);
      setTimeout(() => setCopiedPat(false), 1800);
    } catch {
      // Ignore in restricted environments.
    }
  };

  return (
    <div className="space-y-5">
      {/* Header & KPI Metrics Pills */}
      <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div className="space-y-1">
          <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
            <Blocks className="h-5 w-5 text-xblue" />
            AxAgent 智能体与能力发布
          </h1>
          <p className="text-xs text-muted-foreground">
            多模型路由、REST / Skill / OpenAPI 3.1 能力发布、查询即应用、MCP 双向互联与多智能体协作工作台
          </p>
        </div>

        <div className="flex items-center flex-wrap gap-2">
          <Badge variant="neutral">智能体 {agents.length}</Badge>
          <Badge variant="xblue">PROD 端点 {prodEndpointsCount}</Badge>
          <Badge variant="neutral">查询应用 {queryAppsCount}</Badge>
          <Badge variant="outline">模型源 {providers.length}</Badge>

          <Button
            type="button"
            variant="outline"
            data-testid="open-pat-modal-btn"
            onClick={() => setPatModalOpen(true)}
            className="h-8 px-3"
          >
            <KeyRound className="h-3.5 w-3.5 text-xblue" />
            访问令牌 (PAT)
            {activePat ? (
              <span className="h-1.5 w-1.5 rounded-full bg-xblue" />
            ) : null}
          </Button>
        </div>
      </div>

      {/* 6-Tab Navigation Bar */}
      <div
        role="tablist"
        aria-label="AxAgent 功能页签"
        className="flex items-center gap-1 border-b border-border overflow-x-auto"
      >
        {TABS.map((tab) => {
          const Icon = tab.icon;
          const active = activeTab === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={active}
              data-testid={`axagent-tab-${tab.key}`}
              onClick={() => setActiveTab(tab.key)}
              className={cn(
                "inline-flex items-center gap-1.5 border-b-2 px-3.5 py-2.5 text-xs font-medium transition-colors whitespace-nowrap cursor-pointer",
                active
                  ? "border-xblue text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground"
              )}
            >
              <Icon className={cn("h-3.5 w-3.5", active && "text-xblue")} />
              {tab.label}
            </button>
          );
        })}
      </div>

      {/* Tab Panels */}
      {activeTab === "workbench" ? (
        <ChatWorkbench
          agents={agents}
          selectedAgentId={selectedAgentId}
          onSelectAgentId={setSelectedAgentId}
        />
      ) : activeTab === "agents" ? (
        <AgentEditor
          agents={agents}
          providers={providers}
          endpoints={endpoints}
          mcpClients={mcpClients}
          userPermissions={userPermissions}
          selectedAgentId={selectedAgentId}
          onSelectAgentId={setSelectedAgentId}
          onRefresh={refreshAll}
        />
      ) : activeTab === "apps" ? (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-sm font-semibold">查询即应用画廊 (Query-as-App)</h2>
              <p className="text-xs text-muted-foreground">
                基于端点 JSON Schema 动态生成参数表单，支持当前登录主体直连或 PAT 公开端点测试
              </p>
            </div>
            {activePat ? (
              <Badge variant="xblue">已挂载当前测试 PAT</Badge>
            ) : null}
          </div>

          {appEndpoints.length === 0 ? (
            <Card className="p-10 text-center text-xs text-muted-foreground">
              暂无开启「查询即应用 (show_as_app)」的能力端点，请前往「发布」页签创建或开启
            </Card>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
              {appEndpoints.map((ep) => (
                <AppCard key={ep.id} endpoint={ep} activePat={activePat} />
              ))}
            </div>
          )}
        </div>
      ) : activeTab === "endpoints" ? (
        <EndpointList
          endpoints={endpoints}
          agents={agents}
          metricNames={metricNamesQuery.data ?? []}
          objectTypeNames={objectTypeNamesQuery.data ?? []}
          onRefresh={refreshAll}
        />
      ) : activeTab === "providers" ? (
        <ProviderTable
          providers={providers}
          usage={usageQuery.data ?? []}
          usageFrom={usageFrom}
          usageTo={usageTo}
          onChangeUsageRange={(from, to) => {
            setUsageFrom(from);
            setUsageTo(to);
          }}
          onRefresh={refreshAll}
        />
      ) : (
        <McpClientForm
          mcpClients={mcpClients}
          endpoints={endpoints}
          activePat={activePat}
          onRefresh={refreshAll}
        />
      )}

      {/* Personal Access Token (PAT) Management Modal */}
      {patModalOpen ? (
        <div
          data-testid="pat-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        >
          <Card className="w-full max-w-2xl max-h-[88vh] overflow-y-auto p-5 space-y-4 shadow-lg text-xs">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="flex items-center gap-2">
                <KeyRound className="h-4 w-4 text-xblue" />
                <div>
                  <h2 className="text-sm font-semibold">
                    个人访问令牌管理 (Personal Access Tokens)
                  </h2>
                  <p className="text-[11px] text-muted-foreground">
                    用于调用公开能力端点 (/api/published/*) 与 Streamable HTTP MCP Server (/api/mcp)
                  </p>
                </div>
              </div>
              <Button
                type="button"
                variant="ghost"
                data-testid="close-pat-modal-btn"
                onClick={() => setPatModalOpen(false)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {patError ? (
              <div className="rounded-md border border-border bg-muted px-3 py-2 text-foreground">
                {patError}
              </div>
            ) : null}

            {/* Issue PAT Form */}
            <div className="rounded-md border border-border bg-muted/20 p-3.5 space-y-3">
              <div className="font-semibold text-foreground">签发新访问令牌</div>
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <div className="sm:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    令牌标签 (name)
                  </label>
                  <Input
                    data-testid="pat-name-input"
                    value={patName}
                    onChange={(e) => setPatName(e.target.value)}
                    placeholder="mcp-cursor-client"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    有效天数 (1~365)
                  </label>
                  <Input
                    type="number"
                    min={1}
                    max={365}
                    value={patDays}
                    onChange={(e) => setPatDays(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              </div>

              <div className="space-y-1.5">
                <div className="text-[11px] text-muted-foreground">
                  授权权限子集 (scopes)
                </div>
                <div className="flex flex-wrap gap-2">
                  {userPermissions.map((perm) => (
                    <label
                      key={perm}
                      className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2 py-1 font-mono-tech text-[11px] cursor-pointer"
                    >
                      <input
                        type="checkbox"
                        checked={patScopes.includes(perm)}
                        onChange={() => togglePatScope(perm)}
                        className="accent-[#2563eb]"
                      />
                      {perm}
                    </label>
                  ))}
                </div>
              </div>

              <div className="flex justify-end">
                <Button
                  type="button"
                  data-testid="create-pat-btn"
                  disabled={creatingPat}
                  onClick={() => void handleIssuePat()}
                >
                  {creatingPat ? "签发中…" : "签发令牌"}
                </Button>
              </div>
            </div>

            {/* One-time Plaintext PAT Reveal */}
            {issuedPat ? (
              <div
                data-testid="pat-issued-banner"
                className="rounded-md border border-xblue/40 bg-xblue-soft/40 p-3.5 space-y-2"
              >
                <div className="font-semibold text-foreground">
                  新令牌已生成（仅本次显示明文，已自动挂载为当前页面测试令牌）
                </div>
                <div className="flex items-center gap-2">
                  <code
                    data-testid="pat-plaintext-token"
                    className="flex-1 rounded border border-border bg-background px-2.5 py-1.5 font-mono-tech text-[11px] break-all"
                  >
                    {issuedPat.token}
                  </code>
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => void handleCopyPat(issuedPat.token)}
                    className="h-8 px-2.5 shrink-0"
                  >
                    {copiedPat ? (
                      <Check className="h-3.5 w-3.5 text-xblue" />
                    ) : (
                      <Copy className="h-3.5 w-3.5" />
                    )}
                    复制
                  </Button>
                </div>
              </div>
            ) : null}

            {/* Existing PATs */}
            <div className="space-y-2">
              <div className="font-medium text-foreground">
                已签发令牌列表 ({(patListQuery.data ?? []).length})
              </div>
              {(patListQuery.data ?? []).length === 0 ? (
                <p className="text-muted-foreground py-3 text-center border border-dashed border-border rounded-md">
                  暂无个人访问令牌
                </p>
              ) : (
                <div className="space-y-2">
                  {(patListQuery.data ?? []).map((tok) => {
                    const revoked = Boolean(tok.revoked_at);
                    return (
                      <div
                        key={tok.id}
                        data-testid={`pat-row-${tok.id}`}
                        className="flex items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2"
                      >
                        <div className="space-y-0.5 min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="font-semibold">{tok.name}</span>
                            <span className="font-mono-tech text-[11px] text-muted-foreground">
                              {tok.token_prefix}…
                            </span>
                            <Badge variant={revoked ? "outline" : "xblue"}>
                              {revoked ? "已吊销" : "有效"}
                            </Badge>
                          </div>
                          <div className="text-[11px] text-muted-foreground truncate">
                            Scopes: {tok.scopes.join(", ") || "继承全部"}
                          </div>
                        </div>
                        {!revoked ? (
                          <Button
                            type="button"
                            variant="outline"
                            data-testid={`revoke-pat-btn-${tok.id}`}
                            onClick={() => void handleRevokePat(tok.id)}
                            className="h-7 px-2.5 shrink-0"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                            吊销
                          </Button>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </Card>
        </div>
      ) : null}
    </div>
  );
}
