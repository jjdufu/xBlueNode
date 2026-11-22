import { useState } from "react";
import {
  Edit3,
  Globe,
  KeyRound,
  Lock,
  Plus,
  Power,
  RotateCcw,
  Save,
  Send,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import type {
  CreateNotifyChannelRequest,
  NotifyChannelRecord,
  NotifyKind,
  TestChannelResponse,
  UpdateNotifyChannelRequest,
} from "@/lib/api";
import {
  Badge,
  Button,
  Card,
  Input,
  PasswordInput,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export interface ChannelFormProps {
  channels: readonly NotifyChannelRecord[];
  saving: boolean;
  canEdit: boolean;
  testingChannelId: string | null;
  testResults: Readonly<Record<string, TestChannelResponse>>;
  onCreateChannel: (req: CreateNotifyChannelRequest) => Promise<void>;
  onUpdateChannel: (id: string, req: UpdateNotifyChannelRequest) => Promise<void>;
  onDeleteChannel: (id: string) => Promise<void>;
  onTestChannel: (id: string) => Promise<void>;
}

export function ChannelForm({
  channels,
  saving,
  canEdit,
  testingChannelId,
  testResults,
  onCreateChannel,
  onUpdateChannel,
  onDeleteChannel,
  onTestChannel,
}: ChannelFormProps) {
  const { tx } = useI18n();
  const [editingId, setEditingId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<NotifyKind>("WEBHOOK");
  const [allowPrivateNetwork, setAllowPrivateNetwork] = useState(false);
  const [enabled, setEnabled] = useState(true);

  // Non-secret `config` fields
  const [webhookUrl, setWebhookUrl] = useState("");
  const [webhookHeaderKey, setWebhookHeaderKey] = useState("");
  const [webhookHeaderVal, setWebhookHeaderVal] = useState("");
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState(587);
  const [smtpStarttls, setSmtpStarttls] = useState(true);
  const [emailFrom, setEmailFrom] = useState("");
  const [emailToCsv, setEmailToCsv] = useState("");
  const [dingtalkMobilesCsv, setDingtalkMobilesCsv] = useState("");

  // Write-only `secrets` fields (sealed in `ds_credentials`, never echoed back)
  const [signingSecret, setSigningSecret] = useState("");
  const [smtpUsername, setSmtpUsername] = useState("");
  const [smtpPassword, setSmtpPassword] = useState("");
  const [wecomKey, setWecomKey] = useState("");
  const [dingtalkToken, setDingtalkToken] = useState("");
  const [dingtalkSecret, setDingtalkSecret] = useState("");
  const [feishuWebhookUrl, setFeishuWebhookUrl] = useState("");
  const [feishuSecret, setFeishuSecret] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const channelKinds: readonly { value: NotifyKind; label: string }[] = [
    { value: "WEBHOOK", label: tx("Webhook 签名回调", "Webhook (HMAC-SHA256)") },
    { value: "EMAIL", label: tx("SMTP 邮件", "SMTP Email") },
    { value: "WECOM", label: tx("企业微信机器人", "WeCom Bot") },
    { value: "DINGTALK", label: tx("钉钉机器人", "DingTalk Bot") },
    { value: "FEISHU", label: tx("飞书机器人", "Feishu Bot") },
  ];

  const summarizeChannelConfig = (ch: NotifyChannelRecord): string => {
    switch (ch.kind) {
      case "WEBHOOK":
        return typeof ch.config.url === "string" && ch.config.url
          ? ch.config.url
          : tx("未配置地址", "No URL");
      case "EMAIL": {
        const host =
          typeof ch.config.smtp_host === "string" ? ch.config.smtp_host : "smtp";
        const toList = Array.isArray(ch.config.to) ? ch.config.to.length : 0;
        return `${host} · ×${toList}`;
      }
      case "WECOM":
        return tx("企业微信 Markdown 通道", "WeCom Markdown");
      case "DINGTALK": {
        const mobiles = Array.isArray(ch.config.at_mobiles)
          ? ch.config.at_mobiles.length
          : 0;
        return `${tx("钉钉机器人", "DingTalk")} · @×${mobiles}`;
      }
      case "FEISHU":
        return tx("飞书交互卡片通道", "Feishu Interactive Card");
    }
  };

  const clearSecretInputs = () => {
    setSigningSecret("");
    setSmtpUsername("");
    setSmtpPassword("");
    setWecomKey("");
    setDingtalkToken("");
    setDingtalkSecret("");
    setFeishuWebhookUrl("");
    setFeishuSecret("");
  };

  const resetForm = () => {
    setEditingId(null);
    setName("");
    setKind("WEBHOOK");
    setAllowPrivateNetwork(false);
    setEnabled(true);
    setWebhookUrl("");
    setWebhookHeaderKey("");
    setWebhookHeaderVal("");
    setSmtpHost("");
    setSmtpPort(587);
    setSmtpStarttls(true);
    setEmailFrom("");
    setEmailToCsv("");
    setDingtalkMobilesCsv("");
    clearSecretInputs();
    setFormError(null);
  };

  const loadForEdit = (ch: NotifyChannelRecord) => {
    setEditingId(ch.id);
    setName(ch.name);
    setKind(ch.kind);
    setAllowPrivateNetwork(ch.allow_private_network);
    setEnabled(ch.enabled);
    clearSecretInputs();
    setFormError(null);

    if (ch.kind === "WEBHOOK") {
      setWebhookUrl(typeof ch.config.url === "string" ? ch.config.url : "");
      const hdrs =
        typeof ch.config.headers === "object" && ch.config.headers !== null
          ? (ch.config.headers as Record<string, unknown>)
          : {};
      const firstEntry = Object.entries(hdrs)[0];
      setWebhookHeaderKey(firstEntry ? firstEntry[0] : "");
      setWebhookHeaderVal(
        firstEntry && typeof firstEntry[1] === "string" ? firstEntry[1] : ""
      );
    } else if (ch.kind === "EMAIL") {
      setSmtpHost(
        typeof ch.config.smtp_host === "string" ? ch.config.smtp_host : ""
      );
      setSmtpPort(
        typeof ch.config.smtp_port === "number" ? ch.config.smtp_port : 587
      );
      setSmtpStarttls(ch.config.starttls !== false);
      setEmailFrom(typeof ch.config.from === "string" ? ch.config.from : "");
      setEmailToCsv(
        Array.isArray(ch.config.to)
          ? ch.config.to
              .filter((v): v is string => typeof v === "string")
              .join(", ")
          : ""
      );
    } else if (ch.kind === "DINGTALK") {
      setDingtalkMobilesCsv(
        Array.isArray(ch.config.at_mobiles)
          ? ch.config.at_mobiles
              .filter((v): v is string => typeof v === "string")
              .join(", ")
          : ""
      );
    }
  };

  const buildConfigAndSecrets = (): {
    config: Record<string, unknown>;
    secrets: Record<string, string>;
  } | null => {
    const secrets: Record<string, string> = {};

    if (kind === "WEBHOOK") {
      if (!webhookUrl.trim()) {
        setFormError(tx("Webhook 回调地址不能为空", "Webhook URL is required"));
        return null;
      }
      const headers: Record<string, string> = {};
      if (webhookHeaderKey.trim() && webhookHeaderVal.trim()) {
        headers[webhookHeaderKey.trim()] = webhookHeaderVal.trim();
      }
      if (signingSecret.trim()) {
        secrets.signing_secret = signingSecret.trim();
      }
      return {
        config: { url: webhookUrl.trim(), headers },
        secrets,
      };
    }

    if (kind === "EMAIL") {
      if (!smtpHost.trim() || !emailFrom.trim()) {
        setFormError(
          tx("SMTP 主机与发件人不能为空", "SMTP host and sender are required")
        );
        return null;
      }
      const to = emailToCsv
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      if (smtpUsername.trim()) secrets.smtp_username = smtpUsername.trim();
      if (smtpPassword.trim()) secrets.smtp_password = smtpPassword.trim();
      return {
        config: {
          smtp_host: smtpHost.trim(),
          smtp_port: Math.max(1, Math.trunc(smtpPort)),
          starttls: smtpStarttls,
          from: emailFrom.trim(),
          to,
        },
        secrets,
      };
    }

    if (kind === "WECOM") {
      if (!editingId && !wecomKey.trim()) {
        setFormError(tx("企业微信机器人密钥不能为空", "WeCom key is required"));
        return null;
      }
      if (wecomKey.trim()) secrets.webhook_key = wecomKey.trim();
      return { config: {}, secrets };
    }

    if (kind === "DINGTALK") {
      if (!editingId && !dingtalkToken.trim()) {
        setFormError(tx("钉钉机器人令牌不能为空", "DingTalk token is required"));
        return null;
      }
      const at_mobiles = dingtalkMobilesCsv
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      if (dingtalkToken.trim()) secrets.access_token = dingtalkToken.trim();
      if (dingtalkSecret.trim()) secrets.sign_secret = dingtalkSecret.trim();
      return { config: { at_mobiles }, secrets };
    }

    if (!editingId && !feishuWebhookUrl.trim()) {
      setFormError(tx("飞书机器人地址不能为空", "Feishu webhook URL is required"));
      return null;
    }
    if (feishuWebhookUrl.trim()) secrets.webhook_url = feishuWebhookUrl.trim();
    if (feishuSecret.trim()) secrets.sign_secret = feishuSecret.trim();
    return { config: {}, secrets };
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit) return;
    if (!name.trim()) {
      setFormError(tx("渠道名称不能为空", "Channel name is required"));
      return;
    }
    const built = buildConfigAndSecrets();
    if (!built) return;
    setFormError(null);

    if (editingId) {
      const updatePayload: UpdateNotifyChannelRequest = {
        name: name.trim(),
        kind,
        config: built.config,
        allow_private_network: allowPrivateNetwork,
        enabled,
      };
      if (Object.keys(built.secrets).length > 0) {
        updatePayload.secrets = built.secrets;
      }
      await onUpdateChannel(editingId, updatePayload);
      clearSecretInputs();
    } else {
      await onCreateChannel({
        name: name.trim(),
        kind,
        config: built.config,
        secrets: built.secrets,
        allow_private_network: allowPrivateNetwork,
        enabled,
      });
      resetForm();
    }
  };

  const editingChannel = editingId
    ? channels.find((c) => c.id === editingId) ?? null
    : null;

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
      {/* Left: Channels List */}
      <Card
        className={cn(
          "overflow-hidden",
          canEdit ? "lg:col-span-6" : "lg:col-span-12"
        )}
        data-testid="channels-list-card"
      >
        <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3.5 py-3">
          <div className="flex items-center gap-2 text-xs font-medium">
            <span>{tx("通知渠道", "Notification Channels")}</span>
            <Badge variant="outline">{channels.length}</Badge>
          </div>
          {canEdit && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="new-channel-btn"
              onClick={resetForm}
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新建渠道", "New Channel")}
            </Button>
          )}
        </div>

        <div className="divide-y divide-border">
          {channels.length === 0 ? (
            <div className="p-8 text-center text-xs text-muted-foreground">
              {tx("暂无通知渠道", "No notification channels")}
            </div>
          ) : (
            channels.map((ch) => {
              const testRes = testResults[ch.id];
              const isTesting = testingChannelId === ch.id;
              return (
                <div
                  key={ch.id}
                  data-testid={`channel-card-${ch.id}`}
                  className={cn(
                    "p-3.5 space-y-2.5 text-xs",
                    editingId === ch.id && "bg-xblue-soft/40"
                  )}
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="space-y-1 min-w-0">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="font-semibold text-foreground">
                          {ch.name}
                        </span>
                        <Badge variant="neutral">{ch.kind}</Badge>
                        <StatusBadge
                          status={ch.enabled ? "ENABLED" : "DISABLED"}
                        />
                      </div>
                      <div className="text-[11px] font-mono-tech text-muted-foreground truncate">
                        {summarizeChannelConfig(ch)}
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                        data-testid={`test-channel-btn-${ch.id}`}
                        disabled={isTesting}
                        onClick={() => {
                          void onTestChannel(ch.id);
                        }}
                      >
                        <Send className="h-3 w-3" />
                        {isTesting
                          ? tx("投递中…", "Sending…")
                          : tx("测试投递", "Test")}
                      </Button>

                      {canEdit && (
                        <>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() => {
                              void onUpdateChannel(ch.id, { enabled: !ch.enabled });
                            }}
                            title={
                              ch.enabled
                                ? tx("停用", "Disable")
                                : tx("启用", "Enable")
                            }
                          >
                            <Power className="h-3 w-3" />
                          </Button>

                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            data-testid={`edit-channel-btn-${ch.id}`}
                            onClick={() => loadForEdit(ch)}
                          >
                            <Edit3 className="h-3 w-3" />
                          </Button>

                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="text-red-600 dark:text-red-400"
                            data-testid={`delete-channel-btn-${ch.id}`}
                            onClick={() => {
                              void onDeleteChannel(ch.id);
                            }}
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        </>
                      )}
                    </div>
                  </div>

                  {/* Security & Network Guard Badges */}
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span
                      data-testid={`channel-private-net-badge-${ch.id}`}
                      className={cn(
                        "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono-tech",
                        ch.allow_private_network
                          ? "border border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400"
                          : "border border-border bg-muted text-muted-foreground"
                      )}
                    >
                      {ch.allow_private_network ? (
                        <>
                          <ShieldAlert className="h-2.5 w-2.5" />
                          {tx("允许内网地址", "Private Network Allowed")}
                        </>
                      ) : (
                        <>
                          <Globe className="h-2.5 w-2.5" />
                          {tx("仅公网出站", "Public Network Only")}
                        </>
                      )}
                    </span>

                    <div
                      className="inline-flex flex-wrap items-center gap-1"
                      data-testid={`channel-secrets-badges-${ch.id}`}
                    >
                      {ch.secret_fields_set.length === 0 ? (
                        <Badge variant="outline">
                          {tx("无加密密钥", "No Sealed Secrets")}
                        </Badge>
                      ) : (
                        ch.secret_fields_set.map((field) => (
                          <span
                            key={field}
                            className="inline-flex items-center gap-1 rounded border border-xblue/30 bg-xblue-soft px-1.5 py-0.5 text-[10px] font-mono-tech text-xblue"
                          >
                            <Lock className="h-2.5 w-2.5" />
                            {field} · {tx("已封存", "Sealed")}
                          </span>
                        ))
                      )}
                    </div>
                  </div>

                  {/* Synchronous Test Delivery Result */}
                  {testRes && (
                    <div
                      data-testid={`test-channel-result-${ch.id}`}
                      className={cn(
                        "rounded-md border px-2.5 py-1.5 text-[11px] font-mono-tech flex flex-wrap items-center justify-between gap-2",
                        testRes.ok && testRes.delivery.status === "SENT"
                          ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
                          : "border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400"
                      )}
                    >
                      <span>
                        {tx("状态", "Status")}: {testRes.delivery.status} (
                        {testRes.delivery.event})
                      </span>
                      <span>
                        {testRes.delivery.response_code !== null &&
                          `HTTP ${testRes.delivery.response_code}`}
                        {testRes.delivery.error_message &&
                          ` · ${testRes.delivery.error_message}`}
                      </span>
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      </Card>

      {/* Right: Separated `config` + Write-Only `secrets` Form */}
      {canEdit && (
        <Card className="lg:col-span-6 p-4">
          <form
            onSubmit={(e) => {
              void handleSubmit(e);
            }}
            className="space-y-4"
            data-testid="channel-form"
          >
            <div className="flex items-center justify-between border-b border-border pb-2.5">
              <div className="text-xs font-semibold">
                {editingId
                  ? tx(`编辑渠道 · ${name}`, `Edit Channel · ${name}`)
                  : tx("新建渠道", "New Channel")}
              </div>
              {editingId && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={resetForm}
                >
                  <RotateCcw className="h-3 w-3" />
                  {tx("重置", "Reset")}
                </Button>
              )}
            </div>

            {formError && (
              <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400">
                {formError}
              </div>
            )}

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  {tx("渠道名称", "Channel Name")}
                </label>
                <Input
                  data-testid="channel-name-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="ops_webhook"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>

              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  {tx("渠道类型", "Channel Kind")}
                </label>
                <Select
                  data-testid="channel-kind-select"
                  value={kind}
                  onChange={(e) => setKind(e.target.value as NotifyKind)}
                  className="h-8 w-full"
                >
                  {channelKinds.map((item) => (
                    <option key={item.value} value={item.value}>
                      {item.label}
                    </option>
                  ))}
                </Select>
              </div>
            </div>

            {/* Non-Secret Config Section */}
            <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
              <div className="text-[11px] font-semibold text-foreground">
                {tx("连接参数", "Connection Config")}
              </div>

              {kind === "WEBHOOK" && (
                <div className="space-y-2.5">
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("回调地址", "Webhook URL")}
                    </label>
                    <Input
                      data-testid="channel-webhook-url-input"
                      value={webhookUrl}
                      onChange={(e) => setWebhookUrl(e.target.value)}
                      placeholder="https://hooks.example.com/axalerts"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    <Input
                      value={webhookHeaderKey}
                      onChange={(e) => setWebhookHeaderKey(e.target.value)}
                      placeholder="X-Env"
                      className="h-8 text-xs font-mono-tech"
                    />
                    <Input
                      value={webhookHeaderVal}
                      onChange={(e) => setWebhookHeaderVal(e.target.value)}
                      placeholder="production"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                </div>
              )}

              {kind === "EMAIL" && (
                <div className="space-y-2.5">
                  <div className="grid grid-cols-3 gap-2">
                    <div className="col-span-2">
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("SMTP 主机", "SMTP Host")}
                      </label>
                      <Input
                        value={smtpHost}
                        onChange={(e) => setSmtpHost(e.target.value)}
                        placeholder="smtp.example.com"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("端口", "Port")}
                      </label>
                      <Input
                        type="number"
                        value={smtpPort}
                        onChange={(e) => setSmtpPort(Number(e.target.value))}
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("发件人", "Sender")}
                      </label>
                      <Input
                        value={emailFrom}
                        onChange={(e) => setEmailFrom(e.target.value)}
                        placeholder="alerts@example.com"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("收件人列表", "Recipients")}
                      </label>
                      <Input
                        value={emailToCsv}
                        onChange={(e) => setEmailToCsv(e.target.value)}
                        placeholder="oncall@example.com"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                  <label className="inline-flex items-center gap-2 text-xs cursor-pointer">
                    <input
                      type="checkbox"
                      checked={smtpStarttls}
                      onChange={(e) => setSmtpStarttls(e.target.checked)}
                      className="h-3.5 w-3.5 rounded border-border accent-xblue"
                    />
                    <span>STARTTLS</span>
                  </label>
                </div>
              )}

              {kind === "DINGTALK" && (
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("提醒手机号", "Mentioned Mobiles")}
                  </label>
                  <Input
                    value={dingtalkMobilesCsv}
                    onChange={(e) => setDingtalkMobilesCsv(e.target.value)}
                    placeholder="13800000000"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              )}

              {(kind === "WECOM" || kind === "FEISHU") && (
                <div className="text-[11px] text-muted-foreground">
                  {tx(
                    "机器人敏感凭证仅在下方只写密钥区存储。",
                    "Bot credentials are sealed exclusively in the write-only secrets section."
                  )}
                </div>
              )}
            </div>

            {/* Write-Only Secrets Section */}
            <div className="rounded-md border border-xblue/30 bg-xblue-soft/30 p-3 space-y-2.5">
              <div className="flex items-center justify-between gap-2">
                <span className="inline-flex items-center gap-1.5 text-[11px] font-semibold text-foreground">
                  <KeyRound className="h-3.5 w-3.5 text-xblue" />
                  {tx("只写加密密钥", "Write-Only Sealed Secrets")}
                </span>
                {editingChannel && editingChannel.secret_fields_set.length > 0 && (
                  <span className="text-[10px] font-mono-tech text-xblue">
                    {tx("留空保持原密钥", "Leave blank to keep existing")}
                  </span>
                )}
              </div>

              {kind === "WEBHOOK" && (
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("签名密钥", "Signing Secret")}
                  </label>
                  <PasswordInput
                    autoComplete="new-password"
                    data-testid="channel-signing-secret-input"
                    value={signingSecret}
                    onChange={(e) => setSigningSecret(e.target.value)}
                    placeholder="whsec_..."
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              )}

              {kind === "EMAIL" && (
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("SMTP 用户名", "SMTP Username")}
                    </label>
                    <Input
                      value={smtpUsername}
                      onChange={(e) => setSmtpUsername(e.target.value)}
                      placeholder="mailer"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("SMTP 口令", "SMTP Password")}
                    </label>
                    <PasswordInput
                      autoComplete="new-password"
                      value={smtpPassword}
                      onChange={(e) => setSmtpPassword(e.target.value)}
                      placeholder="••••••••"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                </div>
              )}

              {kind === "WECOM" && (
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("机器人 Key", "Bot Key")}
                  </label>
                  <PasswordInput
                    autoComplete="new-password"
                    value={wecomKey}
                    onChange={(e) => setWecomKey(e.target.value)}
                    placeholder="webhook_key"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              )}

              {kind === "DINGTALK" && (
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("访问令牌", "Access Token")}
                    </label>
                    <PasswordInput
                      autoComplete="new-password"
                      value={dingtalkToken}
                      onChange={(e) => setDingtalkToken(e.target.value)}
                      placeholder="access_token"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("加签密钥", "Signing Secret")}
                    </label>
                    <PasswordInput
                      autoComplete="new-password"
                      value={dingtalkSecret}
                      onChange={(e) => setDingtalkSecret(e.target.value)}
                      placeholder="SEC..."
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                </div>
              )}

              {kind === "FEISHU" && (
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("机器人地址", "Webhook URL")}
                    </label>
                    <PasswordInput
                      autoComplete="new-password"
                      value={feishuWebhookUrl}
                      onChange={(e) => setFeishuWebhookUrl(e.target.value)}
                      placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/..."
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("签名密钥", "Signing Secret")}
                    </label>
                    <PasswordInput
                      autoComplete="new-password"
                      value={feishuSecret}
                      onChange={(e) => setFeishuSecret(e.target.value)}
                      placeholder="sign_secret"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                </div>
              )}
            </div>

            {/* Security Switches & Submit */}
            <div className="flex flex-wrap items-center justify-between gap-2 pt-2 border-t border-border">
              <div className="flex flex-wrap items-center gap-4 text-xs">
                <label className="inline-flex items-center gap-1.5 cursor-pointer">
                  <input
                    type="checkbox"
                    data-testid="channel-private-net-checkbox"
                    checked={allowPrivateNetwork}
                    onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
                    className="h-3.5 w-3.5 rounded border-border accent-xblue"
                  />
                  <span>{tx("允许内网目标", "Allow Private Network")}</span>
                </label>

                <label className="inline-flex items-center gap-1.5 cursor-pointer">
                  <input
                    type="checkbox"
                    data-testid="channel-enabled-checkbox"
                    checked={enabled}
                    onChange={(e) => setEnabled(e.target.checked)}
                    className="h-3.5 w-3.5 rounded border-border accent-xblue"
                  />
                  <span>{tx("启用渠道", "Enable Channel")}</span>
                </label>
              </div>

              <Button
                type="submit"
                variant="primary"
                data-testid="save-channel-btn"
                disabled={saving}
              >
                <Save className="h-3.5 w-3.5" />
                {saving
                  ? tx("保存中…", "Saving…")
                  : editingId
                    ? tx("保存渠道", "Save Channel")
                    : tx("创建渠道", "Create Channel")}
              </Button>
            </div>
          </form>
        </Card>
      )}
    </div>
  );
}
