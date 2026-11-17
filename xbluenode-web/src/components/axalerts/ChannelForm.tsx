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
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface ChannelFormProps {
  channels: readonly NotifyChannelRecord[];
  saving: boolean;
  testingChannelId: string | null;
  testResults: Readonly<Record<string, TestChannelResponse>>;
  onCreateChannel: (req: CreateNotifyChannelRequest) => Promise<void>;
  onUpdateChannel: (id: string, req: UpdateNotifyChannelRequest) => Promise<void>;
  onDeleteChannel: (id: string) => Promise<void>;
  onTestChannel: (id: string) => Promise<void>;
}

const CHANNEL_KINDS: readonly { value: NotifyKind; label: string }[] = [
  { value: "WEBHOOK", label: "WEBHOOK · HMAC-SHA256 签名回调" },
  { value: "EMAIL", label: "EMAIL · SMTP 邮件投递" },
  { value: "WECOM", label: "WECOM · 企业微信群机器人" },
  { value: "DINGTALK", label: "DINGTALK · 钉钉加签机器人" },
  { value: "FEISHU", label: "FEISHU · 飞书交互卡片机器人" },
];

function summarizeChannelConfig(ch: NotifyChannelRecord): string {
  switch (ch.kind) {
    case "WEBHOOK":
      return typeof ch.config.url === "string" && ch.config.url
        ? ch.config.url
        : "未配置 URL";
    case "EMAIL": {
      const host =
        typeof ch.config.smtp_host === "string" ? ch.config.smtp_host : "smtp";
      const toList = Array.isArray(ch.config.to) ? ch.config.to.length : 0;
      return `${host} · 收件人 ×${toList}`;
    }
    case "WECOM":
      return "企业微信群机器人 Markdown 通道";
    case "DINGTALK": {
      const mobiles = Array.isArray(ch.config.at_mobiles)
        ? ch.config.at_mobiles.length
        : 0;
      return `钉钉自定义机器人 · @手机号 ×${mobiles}`;
    }
    case "FEISHU":
      return "飞书自定义机器人 Interactive 卡片";
  }
}

export function ChannelForm({
  channels,
  saving,
  testingChannelId,
  testResults,
  onCreateChannel,
  onUpdateChannel,
  onDeleteChannel,
  onTestChannel,
}: ChannelFormProps) {
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
        setFormError("WEBHOOK 渠道必须填写回调 URL");
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
        setFormError("EMAIL 渠道必须填写 SMTP 主机与发件人地址");
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
        setFormError("新建企业微信渠道必须填写 webhook_key 密钥");
        return null;
      }
      if (wecomKey.trim()) secrets.webhook_key = wecomKey.trim();
      return { config: {}, secrets };
    }

    if (kind === "DINGTALK") {
      if (!editingId && !dingtalkToken.trim()) {
        setFormError("新建钉钉渠道必须填写 access_token 密钥");
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

    // FEISHU
    if (!editingId && !feishuWebhookUrl.trim()) {
      setFormError("新建飞书渠道必须填写完整机器人 webhook_url 秘密地址");
      return null;
    }
    if (feishuWebhookUrl.trim()) secrets.webhook_url = feishuWebhookUrl.trim();
    if (feishuSecret.trim()) secrets.sign_secret = feishuSecret.trim();
    return { config: {}, secrets };
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) {
      setFormError("请填写通知渠道名称");
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
      // Only send `secrets` when new write-only values were entered so existing `ds_credentials` stay intact
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
      <Card className="lg:col-span-6 overflow-hidden" data-testid="channels-list-card">
        <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3.5 py-3">
          <div className="flex items-center gap-2 text-xs font-medium">
            <span>通知投递渠道</span>
            <Badge variant="outline">{channels.length}</Badge>
          </div>
          <Button
            type="button"
            variant="outline"
            className="h-7 px-2.5 text-xs"
            data-testid="new-channel-btn"
            onClick={resetForm}
          >
            <Plus className="h-3.5 w-3.5" />
            新建渠道
          </Button>
        </div>

        <div className="divide-y divide-border">
          {channels.length === 0 ? (
            <div className="p-8 text-center text-xs text-muted-foreground">
              尚未配置通知渠道，支持 Webhook / 邮件 / 企业微信 / 钉钉 / 飞书
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
                        <Badge variant={ch.enabled ? "xblue" : "outline"}>
                          {ch.enabled ? "ENABLED" : "DISABLED"}
                        </Badge>
                      </div>
                      <div className="text-[11px] font-mono-tech text-muted-foreground truncate">
                        {summarizeChannelConfig(ch)}
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0">
                      <Button
                        type="button"
                        variant="outline"
                        className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                        data-testid={`test-channel-btn-${ch.id}`}
                        disabled={isTesting}
                        onClick={() => {
                          void onTestChannel(ch.id);
                        }}
                      >
                        <Send className="h-3 w-3" />
                        {isTesting ? "投递中..." : "测试投递"}
                      </Button>

                      <Button
                        type="button"
                        variant="ghost"
                        className="h-7 px-2 text-[11px]"
                        onClick={() => {
                          void onUpdateChannel(ch.id, { enabled: !ch.enabled });
                        }}
                      >
                        <Power className="h-3 w-3" />
                      </Button>

                      <Button
                        type="button"
                        variant="ghost"
                        className="h-7 px-2 text-[11px]"
                        data-testid={`edit-channel-btn-${ch.id}`}
                        onClick={() => loadForEdit(ch)}
                      >
                        <Edit3 className="h-3 w-3" />
                      </Button>

                      <Button
                        type="button"
                        variant="ghost"
                        className="h-7 px-2 text-[11px] text-red-600 dark:text-red-400"
                        data-testid={`delete-channel-btn-${ch.id}`}
                        onClick={() => {
                          void onDeleteChannel(ch.id);
                        }}
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
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
                          允许内网地址 (allow_private_network)
                        </>
                      ) : (
                        <>
                          <Globe className="h-2.5 w-2.5" />
                          仅公网出站 (NetGuard SSRF 保护)
                        </>
                      )}
                    </span>

                    <div
                      className="inline-flex flex-wrap items-center gap-1"
                      data-testid={`channel-secrets-badges-${ch.id}`}
                    >
                      {ch.secret_fields_set.length === 0 ? (
                        <Badge variant="outline">无加密密钥字段</Badge>
                      ) : (
                        ch.secret_fields_set.map((field) => (
                          <span
                            key={field}
                            className="inline-flex items-center gap-1 rounded border border-xblue/30 bg-xblue-soft px-1.5 py-0.5 text-[10px] font-mono-tech text-xblue"
                          >
                            <Lock className="h-2.5 w-2.5" />
                            {field} · 已封存
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
                        测试状态: {testRes.delivery.status} (event=
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
              {editingId ? `编辑通知渠道 (${name})` : "注册通知渠道"}
            </div>
            {editingId && (
              <Button
                type="button"
                variant="ghost"
                className="h-7 px-2 text-xs"
                onClick={resetForm}
              >
                <RotateCcw className="h-3 w-3" />
                取消编辑
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
                渠道唯一名称 (name)
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
                渠道类型 (kind)
              </label>
              <select
                data-testid="channel-kind-select"
                value={kind}
                onChange={(e) => setKind(e.target.value as NotifyKind)}
                className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
              >
                {CHANNEL_KINDS.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {/* Non-Secret Config Section */}
          <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
            <div className="text-[11px] font-semibold text-foreground">
              1. 非秘密连接参数 (config JSON)
            </div>

            {kind === "WEBHOOK" && (
              <div className="space-y-2.5">
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    回调地址 (config.url)
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
                    placeholder="自定义 Header 名 (可选)"
                    className="h-8 text-xs font-mono-tech"
                  />
                  <Input
                    value={webhookHeaderVal}
                    onChange={(e) => setWebhookHeaderVal(e.target.value)}
                    placeholder="Header 值 (可选)"
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
                      SMTP 主机 (smtp_host)
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
                      端口 (smtp_port)
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
                      发件人 (from)
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
                      收件人列表 (逗号分隔)
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
                  <span>启用 STARTTLS 传输加密</span>
                </label>
              </div>
            )}

            {kind === "DINGTALK" && (
              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  @提醒手机号列表 (at_mobiles，逗号分隔，可选)
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
                {kind} 机器人地址/令牌包含敏感凭证，全部通过下方只写密钥区加密存储（
                <code className="font-mono-tech">config = {"{}"}</code>）。
              </div>
            )}
          </div>

          {/* Write-Only Secrets Section */}
          <div className="rounded-md border border-xblue/30 bg-xblue-soft/30 p-3 space-y-2.5">
            <div className="flex items-center justify-between gap-2">
              <span className="inline-flex items-center gap-1.5 text-[11px] font-semibold text-foreground">
                <KeyRound className="h-3.5 w-3.5 text-xblue" />
                2. 只写加密密钥 (secrets → ds_credentials 密钥环封存)
              </span>
              {editingChannel && editingChannel.secret_fields_set.length > 0 && (
                <span className="text-[10px] font-mono-tech text-xblue">
                  留空则保持已存密钥不变
                </span>
              )}
            </div>

            {kind === "WEBHOOK" && (
              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  HMAC-SHA256 签名密钥 (signing_secret)
                </label>
                <Input
                  type="password"
                  autoComplete="new-password"
                  data-testid="channel-signing-secret-input"
                  value={signingSecret}
                  onChange={(e) => setSigningSecret(e.target.value)}
                  placeholder={
                    editingChannel?.secret_fields_set.includes("signing_secret")
                      ? "•••••••• (已封存，留空不修改)"
                      : "输入签名密钥（永不明文回显）"
                  }
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
            )}

            {kind === "EMAIL" && (
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    SMTP 用户名 (smtp_username)
                  </label>
                  <Input
                    value={smtpUsername}
                    onChange={(e) => setSmtpUsername(e.target.value)}
                    placeholder="留空不修改"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    SMTP 密码 (smtp_password)
                  </label>
                  <Input
                    type="password"
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
                  企业微信机器人 Key (webhook_key)
                </label>
                <Input
                  type="password"
                  autoComplete="new-password"
                  value={wecomKey}
                  onChange={(e) => setWecomKey(e.target.value)}
                  placeholder="输入 key 参数值（永不回显）"
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
            )}

            {kind === "DINGTALK" && (
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    机器人令牌 (access_token)
                  </label>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    value={dingtalkToken}
                    onChange={(e) => setDingtalkToken(e.target.value)}
                    placeholder="access_token"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    加签密钥 (sign_secret)
                  </label>
                  <Input
                    type="password"
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
                    完整机器人地址 (webhook_url)
                  </label>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    value={feishuWebhookUrl}
                    onChange={(e) => setFeishuWebhookUrl(e.target.value)}
                    placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/..."
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    签名校验密钥 (sign_secret 可选)
                  </label>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    value={feishuSecret}
                    onChange={(e) => setFeishuSecret(e.target.value)}
                    placeholder="留空不开启签名"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Flags & Submit */}
          <div className="space-y-2 pt-1">
            <label className="flex items-center gap-2 text-xs cursor-pointer">
              <input
                type="checkbox"
                data-testid="channel-allow-private-checkbox"
                checked={allowPrivateNetwork}
                onChange={(e) => setAllowPrivateNetwork(e.target.checked)}
                className="h-3.5 w-3.5 rounded border-border accent-xblue"
              />
              <span>
                允许访问内网 / Loopback 地址 (
                <code className="font-mono-tech">allow_private_network</code>，需{" "}
                <code className="font-mono-tech">GUARD_ADMIN</code> 权限)
              </span>
            </label>

            <div className="flex items-center justify-between pt-2 border-t border-border">
              <label className="inline-flex items-center gap-2 text-xs cursor-pointer">
                <input
                  type="checkbox"
                  checked={enabled}
                  onChange={(e) => setEnabled(e.target.checked)}
                  className="h-3.5 w-3.5 rounded border-border accent-xblue"
                />
                <span>启用该通知渠道</span>
              </label>

              <Button
                type="submit"
                variant="primary"
                data-testid="save-channel-btn"
                disabled={saving}
              >
                <Save className="h-3.5 w-3.5" />
                {saving
                  ? "保存中..."
                  : editingId
                    ? "更新渠道"
                    : "创建渠道"}
              </Button>
            </div>
          </div>
        </form>
      </Card>
    </div>
  );
}
