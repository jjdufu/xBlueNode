import { useEffect, useMemo, useState } from "react";
import { Check, Copy, KeyRound, Terminal } from "lucide-react";
import type { Permission } from "@/lib/api/enums";
import {
  createApiToken,
  type CreateApiTokenResponse,
  type TokenProfile,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  Tabs,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export const BUILDER_ALLOWED_PERMISSIONS: readonly Permission[] = [
  "DATA_READ",
  "DATA_SYNC",
  "STUDIO_EDIT",
  "FLOW_EDIT",
  "FLOW_RUN",
  "RULE_EDIT",
  "AGENT_EDIT",
  "AGENT_RUN",
  "SIM_RUN",
  "VIEW_EDIT",
];

export const BUILDER_FORBIDDEN_PROD_PERMISSIONS: readonly Permission[] = [
  "STUDIO_PUBLISH",
  "FLOW_PUBLISH",
  "SIM_APPLY",
  "GUARD_ADMIN",
  "OPS_ADMIN",
  "PII_RAW_READ",
];

export type McpBuilderClientKey =
  | "antigravity"
  | "cursor"
  | "claude-code"
  | "codex";

export interface McpBuilderConfigCardProps {
  canIssueToken: boolean;
  userPermissions: readonly Permission[];
  onTokenIssued?: (created: CreateApiTokenResponse) => void;
}

function isValidHttpOriginUrl(raw: string): boolean {
  const trimmed = raw.trim();
  return (
    (trimmed.startsWith("http://") || trimmed.startsWith("https://")) &&
    !/\s/.test(trimmed)
  );
}

export function McpBuilderConfigCard({
  canIssueToken,
  userPermissions,
  onTokenIssued,
}: McpBuilderConfigCardProps) {
  const { tx } = useI18n();

  const defaultOrigin =
    typeof window !== "undefined"
      ? window.location.origin
      : "http://127.0.0.1:5173";
  const mcpEndpointUrl = `${defaultOrigin}/api/mcp`;

  const [profile, setProfile] = useState<TokenProfile>("BUILDER");
  const [tokenName, setTokenName] = useState("");
  const [webBaseUrl, setWebBaseUrl] = useState(defaultOrigin);
  const [clientTab, setClientTab] = useState<McpBuilderClientKey>("antigravity");

  const [issuedToken, setIssuedToken] = useState<string | null>(null);
  const [issuing, setIssuing] = useState(false);
  const [copied, setCopied] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    return () => {
      setIssuedToken(null);
    };
  }, []);

  const effectivePat = issuedToken ?? "<YOUR_BUILDER_PAT>";

  const configSnippet = useMemo(() => {
    if (clientTab === "antigravity" || clientTab === "cursor") {
      return JSON.stringify(
        {
          mcpServers: {
            "xbluenode-axbuilder": {
              serverUrl: mcpEndpointUrl,
              headers: {
                Authorization: `Bearer ${effectivePat}`,
              },
            },
          },
        },
        null,
        2
      );
    }
    if (clientTab === "claude-code") {
      return `claude mcp add --transport http xbluenode-axbuilder ${mcpEndpointUrl} \\\n  --header "Authorization: Bearer ${effectivePat}"`;
    }
    return `[mcp_servers.xbluenode_axbuilder]\nurl = "${mcpEndpointUrl}"\nhttp_headers = { Authorization = "Bearer ${effectivePat}" }`;
  }, [clientTab, effectivePat, mcpEndpointUrl]);

  const handleIssueToken = async () => {
    if (!canIssueToken || issuing) return;
    setErrorMsg(null);

    const effectiveName = tokenName.trim() || "axbuilder-local";
    if (effectiveName.length > 64) {
      setErrorMsg(tx("令牌名称超长", "Token name too long"));
      return;
    }

    const trimmedWebUrl = webBaseUrl.trim();
    if (trimmedWebUrl.length > 0 && !isValidHttpOriginUrl(trimmedWebUrl)) {
      setErrorMsg(tx("前端地址格式无效", "Invalid frontend URL"));
      return;
    }

    const allowedSet = new Set<Permission>(BUILDER_ALLOWED_PERMISSIONS);
    const scopes: Permission[] =
      profile === "BUILDER"
        ? userPermissions.filter((perm) => allowedSet.has(perm))
        : [...userPermissions];

    setIssuing(true);
    try {
      const created = await createApiToken({
        name: effectiveName,
        token_profile: profile,
        ...(trimmedWebUrl.length > 0
          ? { web_base_url: trimmedWebUrl.replace(/\/+$/, "") }
          : {}),
        ...(scopes.length > 0 ? { scopes } : {}),
      });
      setIssuedToken(created.token);
      setTokenName("");
      onTokenIssued?.(created);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("签发失败", "Token issuance failed")
      );
    } finally {
      setIssuing(false);
    }
  };

  const handleCopySnippet = async () => {
    try {
      await navigator.clipboard.writeText(configSnippet);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Ignore in restricted headless environments.
    }
  };

  const clientTabItems = useMemo(
    () => [
      {
        key: "antigravity" as const,
        label: "Antigravity",
        testId: "mcp-builder-tab-antigravity",
      },
      {
        key: "cursor" as const,
        label: "Cursor",
        testId: "mcp-builder-tab-cursor",
      },
      {
        key: "claude-code" as const,
        label: "Claude Code",
        testId: "mcp-builder-tab-claude-code",
      },
      {
        key: "codex" as const,
        label: "Codex",
        testId: "mcp-builder-tab-codex",
      },
    ],
    []
  );

  return (
    <Card
      data-testid="mcp-builder-config-card"
      className="p-5 space-y-4 border-xblue/30"
    >
      {/* Card Header: Title + Live Endpoint Badge only */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Terminal className="h-4 w-4 text-xblue shrink-0" />
          <h2 className="text-sm font-semibold">
            {tx("构建态 MCP 接入", "Builder MCP Setup")}
          </h2>
          <Badge variant="xblue" data-testid="mcp-builder-endpoint-badge">
            {mcpEndpointUrl}
          </Badge>
        </div>
        {issuedToken ? (
          <Badge variant="success" data-testid="mcp-builder-token-ready-badge">
            {tx("已嵌入明文", "Token Embedded")}
          </Badge>
        ) : null}
      </div>

      {errorMsg ? (
        <div
          data-testid="mcp-builder-error"
          className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400"
        >
          {errorMsg}
        </div>
      ) : null}

      {/* Quick Token Issuance Bar */}
      <div className="grid grid-cols-1 md:grid-cols-12 gap-3 items-end rounded-md border border-border bg-muted/20 p-3 text-xs">
        <div className="md:col-span-3 space-y-1">
          <label className="block text-[11px] text-muted-foreground">
            {tx("令牌档位", "Profile")}
          </label>
          <Select
            data-testid="mcp-builder-profile-select"
            aria-label={tx("令牌档位", "Profile")}
            value={profile}
            onChange={(e) =>
              setProfile(
                e.target.value === "GENERAL" ? "GENERAL" : "BUILDER"
              )
            }
            className="w-full"
          >
            <option value="BUILDER">{tx("构建令牌", "Builder")}</option>
            <option value="GENERAL">{tx("通用令牌", "General")}</option>
          </Select>
        </div>

        <div className="md:col-span-3 space-y-1">
          <label className="block text-[11px] text-muted-foreground">
            {tx("令牌名称", "Token Name")}
          </label>
          <Input
            data-testid="mcp-builder-token-name"
            aria-label={tx("令牌名称", "Token Name")}
            value={tokenName}
            onChange={(e) => setTokenName(e.target.value)}
            placeholder="axbuilder-local"
            className="font-mono-tech"
          />
        </div>

        <div className="md:col-span-4 space-y-1">
          <label className="block text-[11px] text-muted-foreground">
            {tx("前端地址", "Frontend URL")}
          </label>
          <Input
            data-testid="mcp-builder-web-url"
            aria-label={tx("前端地址", "Frontend URL")}
            value={webBaseUrl}
            onChange={(e) => setWebBaseUrl(e.target.value)}
            placeholder="http://127.0.0.1:5173"
            className="font-mono-tech"
          />
        </div>

        <div className="md:col-span-2 flex justify-end">
          <Button
            type="button"
            variant="primary"
            data-testid="mcp-builder-issue-btn"
            disabled={!canIssueToken || issuing}
            onClick={() => void handleIssueToken()}
            className="w-full"
          >
            <KeyRound className="h-3.5 w-3.5" />
            {issuing
              ? tx("签发中…", "Issuing…")
              : tx("签发令牌", "Issue Token")}
          </Button>
        </div>
      </div>

      {/* Client Switcher Tabs + Read-only Monospace Snippet */}
      <div className="space-y-2.5">
        <Tabs
          items={clientTabItems}
          value={clientTab}
          onChange={setClientTab}
        />

        <div className="relative">
          <pre
            data-testid="mcp-builder-snippet"
            className="rounded-md border border-border bg-muted/40 p-3 pr-28 font-mono-tech text-[11px] leading-relaxed overflow-x-auto select-all"
          >
            {configSnippet}
          </pre>
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="mcp-builder-copy-btn"
            onClick={() => void handleCopySnippet()}
            className="absolute top-2 right-2"
          >
            {copied ? (
              <Check className="h-3.5 w-3.5 text-xblue" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
            {copied ? tx("已复制", "Copied") : tx("复制配置", "Copy Config")}
          </Button>
        </div>
      </div>
    </Card>
  );
}
