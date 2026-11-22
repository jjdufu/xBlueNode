import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { Check, Copy, ExternalLink, Link2, Trash2 } from "lucide-react";
import type { Permission } from "@/lib/api/enums";
import {
  createAgentEmbed,
  listAgentEmbeds,
  revokeAgentEmbed,
  type AgentRecord,
  type CreateAgentEmbedResponse,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Input,
  Modal,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";

const EMBED_CANDIDATE_PERMISSIONS: readonly Permission[] = [
  "DATA_READ",
  "AGENT_RUN",
  "SIM_RUN",
  "FLOW_RUN",
];

export interface EmbedDialogProps {
  open: boolean;
  agent: AgentRecord | null;
  userPermissions: Permission[];
  onClose: () => void;
}

export function EmbedDialog({
  open,
  agent,
  userPermissions,
  onClose,
}: EmbedDialogProps) {
  const queryClient = useQueryClient();
  const { tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("AGENT_EDIT");

  const [originsText, setOriginsText] = useState("");
  const [rateLimitRpm, setRateLimitRpm] = useState("20");
  const [selectedPermissions, setSelectedPermissions] = useState<Permission[]>([
    "DATA_READ",
    "AGENT_RUN",
  ]);
  const [creating, setCreating] = useState(false);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [createdEmbed, setCreatedEmbed] =
    useState<CreateAgentEmbedResponse | null>(null);
  const [copiedField, setCopiedField] = useState<"url" | "iframe" | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const embedsQuery = useQuery({
    queryKey: ["axagent", "embeds", agent?.id ?? "none"],
    queryFn: () => (agent ? listAgentEmbeds(agent.id) : Promise.resolve([])),
    enabled: open && Boolean(agent),
  });

  const availablePermissions =
    userPermissions.length > 0
      ? EMBED_CANDIDATE_PERMISSIONS.filter((p) => userPermissions.includes(p))
      : EMBED_CANDIDATE_PERMISSIONS;

  const togglePerm = (perm: Permission) => {
    setSelectedPermissions((prev) =>
      prev.includes(perm) ? prev.filter((p) => p !== perm) : [...prev, perm]
    );
  };

  const copyText = useCallback(
    async (text: string, field: "url" | "iframe") => {
      try {
        await navigator.clipboard.writeText(text);
        setCopiedField(field);
        setTimeout(() => setCopiedField(null), 1800);
      } catch {
        // Clipboard API may be unavailable in headless contexts.
      }
    },
    []
  );

  const handleCreate = async () => {
    if (!agent || !canEdit) return;
    setErrorMsg(null);
    const origins = originsText
      .split(/[\n,]/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    const parsedRpm = Number(rateLimitRpm);

    const schema = z.object({
      allowed_origins: z.array(z.string().min(1)),
      rate_limit_rpm: z.number().int().min(1).max(600),
      permissions: z
        .array(z.string())
        .min(1, tx("需勾选至少一项权限", "At least one permission is required")),
    });

    const validated = schema.safeParse({
      allowed_origins: origins,
      rate_limit_rpm: parsedRpm,
      permissions: selectedPermissions,
    });
    if (!validated.success) {
      setErrorMsg(
        validated.error.issues[0]?.message ??
          tx("参数校验失败", "Invalid parameters")
      );
      return;
    }

    setCreating(true);
    try {
      const resp = await createAgentEmbed(agent.id, {
        allowed_origins: validated.data.allowed_origins,
        rate_limit_rpm: validated.data.rate_limit_rpm,
        permissions: selectedPermissions,
      });
      setCreatedEmbed(resp);
      await queryClient.invalidateQueries({
        queryKey: ["axagent", "embeds", agent.id],
      });
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("创建嵌入链接失败", "Failed to create embed link")
      );
    } finally {
      setCreating(false);
    }
  };

  const handleRevoke = async (embedId: string) => {
    if (!agent || !canEdit) return;
    setRevokingId(embedId);
    setErrorMsg(null);
    try {
      await revokeAgentEmbed(embedId);
      if (createdEmbed?.id === embedId) {
        setCreatedEmbed(null);
      }
      await queryClient.invalidateQueries({
        queryKey: ["axagent", "embeds", agent.id],
      });
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("撤销嵌入链接失败", "Failed to revoke embed link")
      );
    } finally {
      setRevokingId(null);
    }
  };

  const fullShareUrl = createdEmbed
    ? `${typeof window !== "undefined" ? window.location.origin : ""}/share/agent/${createdEmbed.token}`
    : "";
  const iframeSnippet = createdEmbed
    ? `<iframe src="${fullShareUrl}" width="100%" height="640" style="border:1px solid #e4e4e7;border-radius:8px;" allow="clipboard-write"></iframe>`
    : "";

  return (
    <Modal
      open={open && Boolean(agent)}
      onClose={onClose}
      maxWidthClass="max-w-2xl"
      testId="embed-dialog"
      title={
        <>
          <Link2 className="h-4 w-4 text-xblue shrink-0" />
          <span>
            {tx(
              `嵌入分享 · ${agent?.name ?? ""}`,
              `Embed Links · ${agent?.name ?? ""}`
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

      {/* Create New Embed Form */}
      {canEdit && (
        <div className="rounded-md border border-border bg-muted/20 p-3.5 space-y-3 text-xs">
          <div className="font-medium text-foreground">
            {tx("签发嵌入链接", "Issue Embed Link")}
          </div>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div className="sm:col-span-2 space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("允许来源域名", "Allowed Origins")}
              </label>
              <Input
                data-testid="embed-origins-input"
                value={originsText}
                onChange={(e) => setOriginsText(e.target.value)}
                placeholder="https://portal.example.com"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("速率限制 (RPM)", "Rate Limit (RPM)")}
              </label>
              <Input
                type="number"
                min={1}
                max={600}
                data-testid="embed-rpm-input"
                value={rateLimitRpm}
                onChange={(e) => setRateLimitRpm(e.target.value)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div className="space-y-1.5">
            <div className="text-[11px] text-muted-foreground">
              {tx("访客权限范围", "Visitor Permissions")}
            </div>
            <div className="flex flex-wrap gap-2">
              {availablePermissions.map((perm) => (
                <label
                  key={perm}
                  title={perm}
                  className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2.5 py-1 text-[11px] cursor-pointer"
                >
                  <input
                    type="checkbox"
                    checked={selectedPermissions.includes(perm)}
                    onChange={() => togglePerm(perm)}
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
              data-testid="create-embed-btn"
              disabled={creating}
              onClick={() => void handleCreate()}
            >
              {creating
                ? tx("签发中…", "Issuing…")
                : tx("生成链接", "Create Link")}
            </Button>
          </div>
        </div>
      )}

      {/* One-time Token Reveal */}
      {createdEmbed ? (
        <div
          data-testid="embed-created-banner"
          className="rounded-md border border-xblue/40 bg-xblue-soft/40 p-3.5 space-y-2.5 text-xs"
        >
          <div className="flex items-center justify-between">
            <span className="font-semibold text-foreground">
              {tx("嵌入令牌已生成（仅本次显示明文）", "Embed token issued (shown once)")}
            </span>
            <a
              href={`/share/agent/${createdEmbed.token}`}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1 text-xblue hover:underline text-[11px]"
            >
              {tx("预览", "Preview")}
              <ExternalLink className="h-3 w-3" />
            </a>
          </div>

          <div className="space-y-1">
            <div className="text-[11px] text-muted-foreground">
              {tx("分享链接", "Share URL")}
            </div>
            <div className="flex items-center gap-2">
              <code
                data-testid="embed-share-url"
                className="flex-1 rounded border border-border bg-background px-2.5 py-1.5 font-mono-tech text-[11px] break-all"
              >
                {fullShareUrl}
              </code>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void copyText(fullShareUrl, "url")}
              >
                {copiedField === "url" ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制链接", "Copy URL")}
              </Button>
            </div>
          </div>

          <div className="space-y-1">
            <div className="text-[11px] text-muted-foreground">
              {tx("iframe 嵌入代码", "iframe Snippet")}
            </div>
            <div className="flex items-start gap-2">
              <pre className="flex-1 rounded border border-border bg-background p-2 font-mono-tech text-[11px] overflow-x-auto whitespace-pre-wrap break-all">
                {iframeSnippet}
              </pre>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void copyText(iframeSnippet, "iframe")}
              >
                {copiedField === "iframe" ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制代码", "Copy Snippet")}
              </Button>
            </div>
          </div>
        </div>
      ) : null}

      {/* Existing Embeds List */}
      <div className="space-y-2">
        <div className="text-xs font-medium text-foreground">
          {tx("已签发链接", "Issued Links")} ({(embedsQuery.data ?? []).length})
        </div>
        {(embedsQuery.data ?? []).length === 0 ? (
          <p className="text-xs text-muted-foreground py-3 text-center border border-dashed border-border rounded-md">
            {tx("暂无嵌入链接", "No embed links")}
          </p>
        ) : (
          <div className="space-y-2">
            {(embedsQuery.data ?? []).map((item) => {
              const revoked = Boolean(item.revoked_at);
              return (
                <div
                  key={item.id}
                  data-testid={`embed-record-${item.id}`}
                  className="flex items-center justify-between gap-3 rounded-md border border-border bg-background px-3 py-2 text-xs"
                >
                  <div className="space-y-1 min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="font-mono-tech font-medium">
                        {item.id.slice(0, 8)}
                      </span>
                      <StatusBadge status={revoked ? "REVOKED" : "ACTIVE"} />
                      <Badge variant="outline">{item.rate_limit_rpm} RPM</Badge>
                    </div>
                    <div className="text-[11px] text-muted-foreground truncate">
                      {item.allowed_origins.length > 0
                        ? item.allowed_origins.join(", ")
                        : tx("仅同源", "Same-Origin")}
                      {" · "}
                      {item.permissions.map((p) => tEnum(p)).join(" · ")}
                    </div>
                  </div>
                  {!revoked && canEdit ? (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`revoke-embed-btn-${item.id}`}
                      disabled={revokingId === item.id}
                      onClick={() => void handleRevoke(item.id)}
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
  );
}
