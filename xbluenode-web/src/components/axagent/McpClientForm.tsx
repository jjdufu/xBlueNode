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
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

const McpClientSchema = z.object({
  name: z
    .string()
    .trim()
    .regex(
      /^[a-z][a-z0-9_]{0,31}$/,
      "客户端名称需以小写字母开头，仅含小写字母、数字与下划线（最长 32 位）"
    ),
  url: z.string().trim().url("MCP Streamable HTTP 地址必须是合法 URL"),
});

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
    setErrorMsg(null);
    setSyncFeedback(null);
    const check = McpClientSchema.safeParse({ name, url });
    if (!check.success) {
      setErrorMsg(check.error.issues[0]?.message ?? "参数校验未通过");
      return;
    }

    let parsedHeaders: Record<string, string> | undefined;
    if (headersJson.trim().length > 0) {
      try {
        const raw: unknown = JSON.parse(headersJson.trim());
        if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
          setErrorMsg("自定义请求头必须是 JSON 字符串键值对象");
          return;
        }
        const map: Record<string, string> = {};
        for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
          map[k] = String(v);
        }
        parsedHeaders = map;
      } catch {
        setErrorMsg("自定义请求头 JSON 格式解析失败");
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
      setErrorMsg(err instanceof Error ? err.message : "登记外部 MCP 服务失败");
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
        `已从 ${synced.name} 同步 ${synced.tools_cache.length} 个远程工具`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "同步远程 MCP 工具失败");
    } finally {
      setSyncingId(null);
    }
  };

  const handleDelete = async (id: string) => {
    setErrorMsg(null);
    try {
      await deleteMcpClient(id);
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "删除 MCP 客户端失败");
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
                本平台内置 MCP Server (/api/mcp)
              </h2>
              <Badge variant="xblue">Streamable HTTP · 2025-06-18</Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              使用个人访问令牌 (Bearer axp_...) 与 Mcp-Session-Id 会话协议连接，每次调用写入 MCP_CALL 审计日志
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            onClick={() => void handleCopyHostConfig()}
          >
            {copiedConfig ? (
              <Check className="h-3.5 w-3.5 text-xblue" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
            复制客户端配置 JSON
          </Button>
        </div>

        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 text-xs">
          <div className="space-y-2.5">
            <div>
              <div className="text-[11px] text-muted-foreground mb-1">
                内置只读工具 (DATA_READ)
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
                动态暴露 PROD 能力端点 ({exposedProdEndpoints.length})
              </div>
              {exposedProdEndpoints.length === 0 ? (
                <span className="text-[11px] text-muted-foreground">
                  暂无开启 expose_mcp 的 PROD 端点
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
        <Card className="lg:col-span-5 p-4 space-y-3 text-xs">
          <div className="flex items-center gap-2 border-b border-border pb-2.5">
            <Plus className="h-4 w-4 text-xblue" />
            <h3 className="text-sm font-semibold">登记外部 MCP 服务</h3>
          </div>

          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              服务短标识 (name: 小写字母与下划线)
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
              自定义认证请求头 JSON（加密存入密钥环，可选）
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
              工具白名单 allowed_tools（逗号分隔，留空允许全部）
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
            <span>允许私网 / 本地回环地址 (需 GUARD_ADMIN)</span>
          </label>

          <div className="flex justify-end pt-1">
            <Button
              type="button"
              data-testid="create-mcp-client-btn"
              disabled={submitting}
              onClick={() => void handleCreate()}
            >
              {submitting ? "登记中…" : "登记 MCP 客户端"}
            </Button>
          </div>
        </Card>

        <div className="lg:col-span-7 space-y-3">
          {mcpClients.length === 0 ? (
            <Card className="p-8 text-center text-xs text-muted-foreground">
              尚未登记外部 MCP 服务客户端
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
                          请求头已封存
                        </Badge>
                      ) : null}
                      <Badge variant={client.enabled ? "xblue" : "outline"}>
                        {client.enabled ? "已启用" : "已停用"}
                      </Badge>
                    </div>
                    <div className="font-mono-tech text-[11px] text-muted-foreground break-all">
                      {client.url}
                    </div>
                  </div>

                  <div className="flex items-center gap-1.5 shrink-0">
                    <Button
                      type="button"
                      variant="outline"
                      data-testid={`sync-mcp-btn-${client.name}`}
                      disabled={syncingId === client.id}
                      onClick={() => void handleSync(client)}
                      className="h-7 px-2.5"
                    >
                      <RefreshCw className="h-3.5 w-3.5 text-xblue" />
                      {syncingId === client.id ? "同步中…" : "同步工具"}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => void handleDelete(client.id)}
                      className="h-7 px-2"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>

                <div
                  data-testid={`mcp-tools-list-${client.name}`}
                  className="border-t border-border pt-2.5 space-y-1.5"
                >
                  <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                    <span>
                      已缓存远程工具 ({client.tools_cache.length})
                    </span>
                    <span className="font-mono-tech">
                      {client.last_synced_at
                        ? `最近同步: ${new Date(client.last_synced_at).toLocaleString()}`
                        : "尚未同步"}
                    </span>
                  </div>
                  {client.tools_cache.length === 0 ? (
                    <p className="text-[11px] text-muted-foreground">
                      点击「同步工具」执行远程 tools/list 发现并缓存工具定义
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
