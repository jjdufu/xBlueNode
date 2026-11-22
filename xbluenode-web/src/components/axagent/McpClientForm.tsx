import { useState } from "react";
import { z } from "zod";
import {
  Check,
  Copy,
  KeyRound,
  Plug,
  Plus,
  RefreshCw,
  Server,
  Trash2,
} from "lucide-react";
import {
  createMcpClient,
  deleteMcpClient,
  syncMcpClient,
  type EndpointSpec,
  type McpClientRecord,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Card,
  Input,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";

const BUILTIN_MCP_TOOLS = [
  "search_knowledge",
  "ask_data",
  "list_datasets",
  "describe_dataset",
  "query_objects",
  "graph_neighbors",
  "get_node",
];

export interface McpClientFormProps {
  mcpClients: McpClientRecord[];
  endpoints: EndpointSpec[];
  activePat?: string | null;
  onRefresh: () => Promise<void>;
}

export function McpClientForm({
  mcpClients,
  endpoints,
  activePat = null,
  onRefresh,
}: McpClientFormProps) {
  const { tx, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("AGENT_EDIT");

  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [headersJson, setHeadersJson] = useState("");
  const [allowedToolsText, setAllowedToolsText] = useState("");
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);

  const [submitting, setSubmitting] = useState(false);
  const [syncingId, setSyncingId] = useState<string | null>(null);
  const [copiedConfig, setCopiedConfig] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [syncFeedback, setSyncFeedback] = useState<string | null>(null);

  const origin =
    typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:9995";
  const mcpServerUrl = `${origin}/api/mcp`;
  const exposedProdEndpoints = endpoints.filter(
    (e) => e.environment === "PROD" && e.expose_mcp && e.enabled
  );

  const hostConfigSnippet = JSON.stringify(
    {
      mcpServers: {
        axpoint: {
          type: "streamable-http",
          url: mcpServerUrl,
          headers: {
            Authorization: `Bearer ${
              activePat && activePat.startsWith("axp_")
                ? activePat
                : "axp_<YOUR_PERSONAL_ACCESS_TOKEN>"
            }`,
          },
        },
      },
    },
    null,
    2
  );

  const handleCopyHostConfig = async () => {
    try {
      await navigator.clipboard.writeText(hostConfigSnippet);
      setCopiedConfig(true);
      setTimeout(() => setCopiedConfig(false), 1800);
    } catch {
      // Ignore in restricted headless contexts.
    }
  };

  const handleCreate = async () => {
    if (!canEdit) return;
    setErrorMsg(null);
    setSyncFeedback(null);

    const mcpClientSchema = z.object({
      name: z
        .string()
        .trim()
        .regex(
          /^[a-z][a-z0-9_]{0,31}$/,
          tx(
            "服务标识需以小写字母开头，仅含小写字母、数字与下划线",
            "Name must start with a lowercase letter and contain only a-z, 0-9, _"
          )
        ),
      url: z
        .string()
        .trim()
        .url(tx("MCP 服务地址须为合法 URL", "MCP URL must be valid")),
    });

    const check = mcpClientSchema.safeParse({ name, url });
    if (!check.success) {
      setErrorMsg(
        check.error.issues[0]?.message ??
          tx("参数校验未通过", "Invalid parameters")
      );
      return;
    }

    let parsedHeaders: Record<string, string> | undefined;
    if (headersJson.trim().length > 0) {
      try {
        const raw: unknown = JSON.parse(headersJson.trim());
        if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
          setErrorMsg(
            tx("请求头须为 JSON 键值对象", "Headers must be a JSON object")
          );
          return;
        }
        const map: Record<string, string> = {};
        for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
          map[k] = String(v);
        }
        parsedHeaders = map;
      } catch {
        setErrorMsg(tx("请求头 JSON 解析失败", "Invalid headers JSON"));
        return;
      }
    }

    const allowedTools = allowedToolsText
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    setSubmitting(true);
    try {
      await createMcpClient({
        name: check.data.name,
        url: check.data.url,
        ...(parsedHeaders ? { headers: parsedHeaders } : {}),
        allowed_tools: allowedTools,
        allow_private_network: allowPrivateNetwork,
        enabled: true,
      });
      setName("");
      setUrl("");
      setHeadersJson("");
      setAllowedToolsText("");
      setAllowPrivateNetwork(false);
      await onRefresh();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("登记外部 MCP 失败", "Failed to register MCP client")
      );
    } finally {
      setSubmitting(false);
    }
  };

  const handleSync = async (client: McpClientRecord) => {
    setSyncingId(client.id);
    setErrorMsg(null);
    setSyncFeedback(null);
    try {
      const synced = await syncMcpClient(client.id);
      await onRefresh();
      setSyncFeedback(
        tx(
          `已从 ${synced.name} 同步 ${synced.tools_cache.length} 个远程工具`,
          `Synced ${synced.tools_cache.length} tools from ${synced.name}`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("同步远程工具失败", "Failed to sync remote tools")
      );
    } finally {
      setSyncingId(null);
    }
  };

  const handleDelete = async (id: string) => {
    if (!canEdit) return;
    setErrorMsg(null);
    try {
      await deleteMcpClient(id);
      await onRefresh();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("删除失败", "Failed to delete MCP client")
      );
    }
  };

  return (
    <div className="space-y-6">
      {/* 1. Built-in AxPoint MCP Server Card */}
      <Card
        data-testid="builtin-mcp-server-card"
        className="p-5 space-y-4 border-xblue/30"
      >
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <Server className="h-4 w-4 text-xblue" />
              <h2 className="text-sm font-semibold">
                {tx("内置 MCP 服务 (/api/mcp)", "Built-in MCP Server (/api/mcp)")}
              </h2>
              <Badge variant="xblue">Streamable HTTP · 2025-06-18</Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              {tx(
                "使用个人访问令牌 (Bearer axp_...) 接入，所有调用写入审计日志。",
                "Authenticate via Personal Access Token (Bearer axp_...); every call is audited."
              )}
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void handleCopyHostConfig()}
          >
            {copiedConfig ? (
              <Check className="h-3.5 w-3.5 text-xblue" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
            {tx("复制配置", "Copy Config")}
          </Button>
        </div>

        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 text-xs">
          <div className="space-y-2.5">
            <div>
              <div className="text-[11px] text-muted-foreground mb-1">
                {tx("内置只读工具", "Built-in Read Tools")}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {BUILTIN_MCP_TOOLS.map((t) => (
                  <Badge key={t} variant="neutral">
                    {t}
                  </Badge>
                ))}
              </div>
            </div>

            <div>
              <div className="text-[11px] text-muted-foreground mb-1">
                {tx("生产环境暴露端点", "Exposed Production Endpoints")} (
                {exposedProdEndpoints.length})
              </div>
              {exposedProdEndpoints.length === 0 ? (
                <span className="text-[11px] text-muted-foreground">
                  {tx("暂无已暴露端点", "No exposed endpoints")}
                </span>
              ) : (
                <div className="flex flex-wrap gap-1.5">
                  {exposedProdEndpoints.map((ep) => (
                    <Badge key={ep.id} variant="xblue">
                      {ep.slug.replace(/-/g, "_")}
                    </Badge>
                  ))}
                </div>
              )}
            </div>
          </div>

          <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto">
            {hostConfigSnippet}
          </pre>
        </div>
      </Card>

      {errorMsg ? (
        <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
          {errorMsg}
        </div>
      ) : null}
      {syncFeedback ? (
        <div
          data-testid="mcp-sync-feedback"
          className="rounded-md border border-xblue/40 bg-xblue-soft/40 px-3 py-2 text-xs text-foreground"
        >
          {syncFeedback}
        </div>
      ) : null}

      {/* 2. External MCP Server Registration Form + Discovered Client Cards */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        {canEdit && (
          <Card className="lg:col-span-5 p-4 space-y-3 text-xs">
            <div className="flex items-center gap-2 border-b border-border pb-2.5">
              <Plus className="h-4 w-4 text-xblue" />
              <h3 className="text-sm font-semibold">
                {tx("登记外部 MCP 服务", "Register External MCP Server")}
              </h3>
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("服务标识", "Client Name")}
              </label>
              <Input
                data-testid="mcp-name-input"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="logistics_mcp"
                className="font-mono-tech text-xs"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                Streamable HTTP URL
              </label>
              <Input
                data-testid="mcp-url-input"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://mcp.partner.example.com/mcp"
                className="font-mono-tech text-xs"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("认证请求头 JSON", "Auth Headers JSON")}
              </label>
              <Textarea
                data-testid="mcp-headers-input"
                rows={2}
                value={headersJson}
                onChange={(e) => setHeadersJson(e.target.value)}
                placeholder='{"Authorization": "Bearer secret_token"}'
                className="font-mono-tech text-xs"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("工具允许名单", "Allowed Tools")}
              </label>
              <Input
                value={allowedToolsText}
                onChange={(e) => setAllowedToolsText(e.target.value)}
                placeholder="get_forecast, track_shipment"
                className="font-mono-tech text-xs"
              />
            </div>

            <label className="inline-flex items-center gap-2 cursor-pointer pt-1">
              <input
                type="checkbox"
                checked={allowPrivateNetwork}
                onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
                className="accent-[#2563eb]"
              />
              <span>{tx("允许私网地址", "Allow Private Network")}</span>
            </label>

            <div className="flex justify-end pt-1">
              <Button
                type="button"
                data-testid="create-mcp-client-btn"
                disabled={submitting}
                onClick={() => void handleCreate()}
              >
                {submitting
                  ? tx("登记中…", "Registering…")
                  : tx("登记服务", "Register")}
              </Button>
            </div>
          </Card>
        )}

        <div
          className={cn(
            "space-y-3",
            canEdit ? "lg:col-span-7" : "lg:col-span-12"
          )}
        >
          {mcpClients.length === 0 ? (
            <Card className="p-8 text-center text-xs text-muted-foreground">
              {tx("暂无外部 MCP 客户端", "No external MCP clients")}
            </Card>
          ) : (
            mcpClients.map((client) => (
              <Card
                key={client.id}
                data-testid={`mcp-client-card-${client.name}`}
                className="p-4 space-y-3 text-xs"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="space-y-0.5 min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                      <Plug className="h-3.5 w-3.5 text-xblue shrink-0" />
                      <span className="font-mono-tech font-semibold text-sm">
                        {client.name}
                      </span>
                      {client.has_headers ? (
                        <Badge variant="outline">
                          <KeyRound className="h-2.5 w-2.5 mr-1" />
                          {tx("请求头已封存", "Headers Sealed")}
                        </Badge>
                      ) : null}
                      <StatusBadge
                        status={client.enabled ? "ENABLED" : "DISABLED"}
                      />
                    </div>
                    <div className="font-mono-tech text-[11px] text-muted-foreground break-all">
                      {client.url}
                    </div>
                  </div>

                  <div className="flex items-center gap-1.5 shrink-0">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`sync-mcp-btn-${client.name}`}
                      disabled={syncingId === client.id}
                      onClick={() => void handleSync(client)}
                    >
                      <RefreshCw className="h-3.5 w-3.5 text-xblue" />
                      {syncingId === client.id
                        ? tx("同步中…", "Syncing…")
                        : tx("同步工具", "Sync Tools")}
                    </Button>
                    {canEdit && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => void handleDelete(client.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    )}
                  </div>
                </div>

                <div
                  data-testid={`mcp-tools-list-${client.name}`}
                  className="border-t border-border pt-2.5 space-y-1.5"
                >
                  <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                    <span>
                      {tx("已缓存工具", "Cached Tools")} (
                      {client.tools_cache.length})
                    </span>
                    <span className="font-mono-tech">
                      {client.last_synced_at
                        ? fmtTime(client.last_synced_at)
                        : tx("尚未同步", "Not synced")}
                    </span>
                  </div>
                  {client.tools_cache.length === 0 ? (
                    <p className="text-[11px] text-muted-foreground">
                      {tx("同步后加载远程工具清单", "Sync to load remote tool definitions")}
                    </p>
                  ) : (
                    <div className="flex flex-wrap gap-1.5">
                      {client.tools_cache.map((tool) => (
                        <Badge
                          key={tool.name}
                          variant="xblue"
                          title={tool.description}
                        >
                          {tool.name}
                        </Badge>
                      ))}
                    </div>
                  )}
                </div>
              </Card>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
