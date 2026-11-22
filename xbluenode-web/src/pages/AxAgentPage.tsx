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
import {
  Badge,
  Button,
  Card,
  Input,
  Modal,
  PageHeader,
  StatusBadge,
  Tabs,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { usePrincipal } from "@/lib/permissions";

type AxAgentTabKey =
  | "workbench"
  | "agents"
  | "apps"
  | "endpoints"
  | "providers"
  | "mcp";

const DEFAULT_PAT_SCOPES: readonly Permission[] = [
  "DATA_READ",
  "AGENT_RUN",
  "FLOW_RUN",
  "SIM_RUN",
];

export function AxAgentPage() {
  const queryClient = useQueryClient();
  const { tx, tEnum, fmtNum } = useI18n();
  const { canAny } = usePrincipal();
  const canManagePat = canAny(["AGENT_RUN", "AGENT_EDIT", "DATA_READ"]);

  const [activeTab, setActiveTab] = useState<AxAgentTabKey>("workbench");
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);

  // Active testing PAT stored only in memory (never written to localStorage)
  const [activePat, setActivePat] = useState<string | null>(null);
  const [patModalOpen, setPatModalOpen] = useState(false);
  const [patName, setPatName] = useState("");
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
    if (!canManagePat) return;
    setPatError(null);
    const patFormSchema = z.object({
      name: z
        .string()
        .trim()
        .min(1, tx("令牌名称不能为空", "Token name is required"))
        .max(64),
      scopes: z
        .array(z.string())
        .min(1, tx("至少勾选一项权限", "At least one scope is required")),
      expires_in_days: z.number().int().min(1).max(365),
    });

    const check = patFormSchema.safeParse({
      name: patName,
      scopes: patScopes,
      expires_in_days: Number(patDays),
    });
    if (!check.success) {
      setPatError(
        check.error.issues[0]?.message ??
          tx("参数校验失败", "Invalid parameters")
      );
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
      setPatName("");
      await queryClient.invalidateQueries({ queryKey: ["axagent", "pat-list"] });
    } catch (err) {
      setPatError(
        err instanceof Error
          ? err.message
          : tx("签发令牌失败", "Failed to issue token")
      );
    } finally {
      setCreatingPat(false);
    }
  };

  const handleRevokePat = async (id: string) => {
    if (!canManagePat) return;
    setPatError(null);
    try {
      await revokeApiToken(id);
      if (issuedPat?.id === id) {
        setIssuedPat(null);
        setActivePat(null);
      }
      await queryClient.invalidateQueries({ queryKey: ["axagent", "pat-list"] });
    } catch (err) {
      setPatError(
        err instanceof Error
          ? err.message
          : tx("撤销令牌失败", "Failed to revoke token")
      );
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

  const tabItems = useMemo(
    () => [
      {
        key: "workbench" as const,
        label: tx("工作台", "Workbench"),
        icon: <MessageSquare className="h-3.5 w-3.5" />,
        testId: "axagent-tab-workbench",
      },
      {
        key: "agents" as const,
        label: tx("智能体", "Agents"),
        icon: <Bot className="h-3.5 w-3.5" />,
        count: agents.length,
        testId: "axagent-tab-agents",
      },
      {
        key: "apps" as const,
        label: tx("应用", "Apps"),
        icon: <LayoutGrid className="h-3.5 w-3.5" />,
        count: queryAppsCount,
        testId: "axagent-tab-apps",
      },
      {
        key: "endpoints" as const,
        label: tx("发布", "Endpoints"),
        icon: <Globe className="h-3.5 w-3.5" />,
        count: endpoints.length,
        testId: "axagent-tab-endpoints",
      },
      {
        key: "providers" as const,
        label: tx("模型", "Models"),
        icon: <Cpu className="h-3.5 w-3.5" />,
        count: providers.length,
        testId: "axagent-tab-providers",
      },
      {
        key: "mcp" as const,
        label: "MCP",
        icon: <Plug className="h-3.5 w-3.5" />,
        count: mcpClients.length,
        testId: "axagent-tab-mcp",
      },
    ],
    [
      agents.length,
      endpoints.length,
      mcpClients.length,
      providers.length,
      queryAppsCount,
      tx,
    ]
  );

  return (
    <div className="space-y-5">
      <PageHeader
        title={
          <span className="inline-flex items-center gap-2">
            <Blocks className="h-4 w-4 text-xblue shrink-0" />
            {tx(MODULE_META.axagent.label, MODULE_META.axagent.enLabel)}
          </span>
        }
        badges={
          <div className="flex items-center flex-wrap gap-1.5">
            <Badge variant="neutral">
              {tx("智能体", "Agents")} {fmtNum(agents.length, 0)}
            </Badge>
            <Badge variant="xblue">
              {tx("生产端点", "Prod")} {fmtNum(prodEndpointsCount, 0)}
            </Badge>
            <Badge variant="outline">
              {tx("模型源", "Providers")} {fmtNum(providers.length, 0)}
            </Badge>
          </div>
        }
        actions={
          canManagePat ? (
            <Button
              type="button"
              variant="outline"
              data-testid="open-pat-modal-btn"
              onClick={() => setPatModalOpen(true)}
            >
              <KeyRound className="h-3.5 w-3.5 text-xblue" />
              {tx("访问令牌", "API Tokens")}
              {activePat ? (
                <span className="h-1.5 w-1.5 rounded-full bg-xblue" />
              ) : null}
            </Button>
          ) : undefined
        }
      />

      {/* 6-Tab Navigation Bar */}
      <Tabs items={tabItems} value={activeTab} onChange={setActiveTab} />

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
            <h2 className="text-sm font-semibold">
              {tx("查询即应用", "Query-as-App Gallery")}
            </h2>
            {activePat ? (
              <Badge variant="xblue">
                {tx("已挂载测试令牌", "PAT Active")}
              </Badge>
            ) : null}
          </div>

          {appEndpoints.length === 0 ? (
            <Card className="p-10 text-center text-xs text-muted-foreground">
              {tx("暂无查询应用", "No query apps published")}
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
      <Modal
        open={patModalOpen}
        onClose={() => setPatModalOpen(false)}
        maxWidthClass="max-w-2xl"
        testId="pat-modal"
        closeTestId="close-pat-modal-btn"
        title={
          <>
            <KeyRound className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("个人访问令牌", "Personal Access Tokens")}</span>
          </>
        }
      >
        {patError ? (
          <div className="rounded-md border border-border bg-muted px-3 py-2 text-foreground">
            {patError}
          </div>
        ) : null}

        {/* Issue PAT Form */}
        <div className="rounded-md border border-border bg-muted/20 p-3.5 space-y-3">
          <div className="font-semibold text-foreground">
            {tx("签发访问令牌", "Issue Token")}
          </div>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div className="sm:col-span-2 space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("令牌名称", "Token Name")}
              </label>
              <Input
                data-testid="pat-name-input"
                value={patName}
                onChange={(e) => setPatName(e.target.value)}
                placeholder="ci_deploy_token"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("有效天数", "Validity (days)")}
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
              {tx("权限范围", "Permission Scopes")}
            </div>
            <div className="flex flex-wrap gap-2">
              {userPermissions.map((perm) => (
                <label
                  key={perm}
                  title={perm}
                  className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2 py-1 text-[11px] cursor-pointer"
                >
                  <input
                    type="checkbox"
                    checked={patScopes.includes(perm)}
                    onChange={() => togglePatScope(perm)}
                    className="accent-[#2563eb]"
                  />
                  <span>{tEnum(perm)}</span>
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
              {creatingPat
                ? tx("签发中…", "Issuing…")
                : tx("签发令牌", "Issue Token")}
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
              {tx(
                "令牌已生成（仅本次显示明文）",
                "Token issued (plaintext shown once)"
              )}
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
                size="sm"
                onClick={() => void handleCopyPat(issuedPat.token)}
              >
                {copiedPat ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制", "Copy")}
              </Button>
            </div>
          </div>
        ) : null}

        {/* Existing PATs */}
        <div className="space-y-2">
          <div className="font-medium text-foreground">
            {tx("已签发令牌", "Issued Tokens")} (
            {(patListQuery.data ?? []).length})
          </div>
          {(patListQuery.data ?? []).length === 0 ? (
            <p className="text-muted-foreground py-3 text-center border border-dashed border-border rounded-md">
              {tx("暂无访问令牌", "No personal access tokens")}
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
                        <StatusBadge
                          status={revoked ? "REVOKED" : "ACTIVE"}
                        />
                      </div>
                      <div className="text-[11px] text-muted-foreground truncate">
                        {tok.scopes.length > 0
                          ? tok.scopes.map((s) => tEnum(s)).join(" · ")
                          : tx("继承全部权限", "All permissions")}
                      </div>
                    </div>
                    {!revoked ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        data-testid={`revoke-pat-btn-${tok.id}`}
                        onClick={() => void handleRevokePat(tok.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        {tx("撤销", "Revoke")}
                      </Button>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </Modal>
    </div>
  );
}
