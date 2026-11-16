import { useState } from "react";
import { Check, Copy, KeyRound, Webhook } from "lucide-react";
import { axflowApi, type TriggerCfg } from "@/lib/api/axflow";
import type { OutboxTopic, TriggerType } from "@/lib/api/enums";
import { Badge, Button, Input } from "@/components/ui/primitives";

const OUTBOX_TOPICS: readonly OutboxTopic[] = [
  "dataset.synced",
  "node.metrics_changed",
  "node.upserted",
  "approval.decided",
  "sim.applied",
  "alert.opened",
  "alert.resolved",
  "export.finished",
];

const CRON_PRESETS = [
  { label: "每小时整点", expr: "0 */1 * * *" },
  { label: "每日 02:00", expr: "0 2 * * *" },
  { label: "每 15 分钟", expr: "*/15 * * * *" },
] as const;

export interface TriggerEditorProps {
  flowId: string | null;
  slug: string | null;
  triggerCfg: TriggerCfg;
  tenantId: string;
  onChangeTrigger: (next: TriggerCfg) => void;
  onChangeSlug: (nextSlug: string) => void;
}

export function TriggerEditor({
  flowId,
  slug,
  triggerCfg,
  tenantId,
  onChangeTrigger,
  onChangeSlug,
}: TriggerEditorProps) {
  const [rotating, setRotating] = useState(false);
  const [issuedSecret, setIssuedSecret] = useState<string | null>(null);
  const [hookError, setHookError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const webhookPath = `/api/axflow/hooks/${tenantId || "default"}/${
    slug && slug.trim().length > 0 ? slug.trim() : "<slug>"
  }`;

  const handleRotateSecret = async () => {
    if (!flowId) return;
    setRotating(true);
    setHookError(null);
    try {
      const res = await axflowApi.rotateWebhookSecret(flowId);
      setIssuedSecret(res.secret);
    } catch (err) {
      setHookError(err instanceof Error ? err.message : String(err));
    } finally {
      setRotating(false);
    }
  };

  const copySecret = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // clipboard unavailable
    }
  };

  return (
    <div className="space-y-3.5 text-xs" data-testid="axflow-trigger-editor">
      <div>
        <label className="block font-medium mb-1">触发器类型 (type)</label>
        <select
          data-testid="trigger-type-select"
          value={triggerCfg.type}
          onChange={(e) => {
            const nextType = e.target.value as TriggerType;
            if (nextType === "CRON") {
              onChangeTrigger({
                type: "CRON",
                cron: triggerCfg.cron ?? "0 */1 * * *",
                timezone: triggerCfg.timezone ?? "Asia/Shanghai",
                catch_up: Boolean(triggerCfg.catch_up),
              });
            } else if (nextType === "OUTBOX_EVENT") {
              onChangeTrigger({
                type: "OUTBOX_EVENT",
                topic: triggerCfg.topic ?? "dataset.synced",
                filter: triggerCfg.filter ?? "",
              });
            } else {
              onChangeTrigger({ type: nextType });
            }
          }}
          className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
        >
          <option value="MANUAL">MANUAL · 手动 / API 触发</option>
          <option value="CRON">CRON · 定时调度 (PROD)</option>
          <option value="OUTBOX_EVENT">OUTBOX_EVENT · 平台事件总线 (PROD)</option>
          <option value="WEBHOOK">WEBHOOK · 外部 HTTP 回调 (PROD)</option>
          <option value="RULE">RULE · 运营告警规则联动</option>
          <option value="API">API · 已发布数据服务触发</option>
          <option value="MCP">MCP · 智能体工具调用触发</option>
          <option value="SIM_APPLY">SIM_APPLY · 因果推演决策下发</option>
        </select>
      </div>

      {triggerCfg.type === "CRON" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">Cron 表达式</label>
            <Input
              data-testid="trigger-cron-input"
              value={triggerCfg.cron ?? ""}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, cron: e.target.value })
              }
              placeholder="0 */1 * * *"
              className="h-8 font-mono-tech text-xs"
            />
            <div className="flex flex-wrap gap-1 mt-1.5">
              {CRON_PRESETS.map((p) => (
                <button
                  key={p.expr}
                  type="button"
                  onClick={() =>
                    onChangeTrigger({ ...triggerCfg, cron: p.expr })
                  }
                  className="rounded border border-border bg-card px-1.5 py-0.5 text-[10px] hover:border-xblue cursor-pointer"
                >
                  {p.label} ({p.expr})
                </button>
              ))}
            </div>
          </div>

          <div>
            <label className="block font-medium mb-1">时区 (timezone)</label>
            <Input
              data-testid="trigger-timezone-input"
              value={triggerCfg.timezone ?? "Asia/Shanghai"}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, timezone: e.target.value })
              }
              placeholder="Asia/Shanghai"
              className="h-8 font-mono-tech text-xs"
            />
          </div>

          <label className="flex items-center justify-between pt-1 cursor-pointer">
            <span>补跑错过的历史周期 (catch_up)</span>
            <input
              data-testid="trigger-catchup-checkbox"
              type="checkbox"
              checked={Boolean(triggerCfg.catch_up)}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, catch_up: e.target.checked })
              }
              className="h-3.5 w-3.5 accent-blue-600"
            />
          </label>
        </div>
      )}

      {triggerCfg.type === "OUTBOX_EVENT" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">
              订阅主题 (OutboxTopic)
            </label>
            <select
              data-testid="trigger-topic-select"
              value={triggerCfg.topic ?? "dataset.synced"}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, topic: e.target.value })
              }
              className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
            >
              {OUTBOX_TOPICS.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="block font-medium mb-1">
              载荷过滤表达式 (filter, 可选)
            </label>
            <Input
              data-testid="trigger-filter-input"
              value={triggerCfg.filter ?? ""}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, filter: e.target.value })
              }
              placeholder="例如: rows_written > 0"
              className="h-8 font-mono-tech text-xs"
            />
          </div>
        </div>
      )}

      {triggerCfg.type === "WEBHOOK" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">
              流程短标识 (slug)
            </label>
            <Input
              data-testid="trigger-slug-input"
              value={slug ?? ""}
              onChange={(e) => onChangeSlug(e.target.value)}
              placeholder="如: daily-supply-hook"
              className="h-8 font-mono-tech text-xs"
            />
          </div>

          <div>
            <div className="flex items-center gap-1 text-[11px] font-medium mb-1">
              <Webhook className="h-3 w-3 text-xblue" />
              外部回调地址 (POST)
            </div>
            <div
              data-testid="trigger-webhook-url"
              className="rounded border border-border bg-card px-2.5 py-1.5 font-mono-tech text-[11px] break-all"
            >
              {webhookPath}
            </div>
          </div>

          <div className="pt-1 space-y-2">
            <Button
              type="button"
              variant="outline"
              className="w-full h-8 text-xs"
              data-testid="rotate-webhook-secret-btn"
              disabled={!flowId || rotating}
              onClick={() => void handleRotateSecret()}
            >
              <KeyRound className="h-3.5 w-3.5 text-xblue" />
              {rotating ? "正在生成密钥..." : "生成/轮换密钥"}
            </Button>

            {hookError && (
              <div className="text-[11px] text-red-600 dark:text-red-400">
                {hookError}
              </div>
            )}

            {issuedSecret && (
              <div
                data-testid="webhook-secret-box"
                className="rounded border border-xblue/40 bg-xblue-soft p-2.5 space-y-1.5"
              >
                <div className="flex items-center justify-between">
                  <Badge variant="xblue">一次性明文 Bearer 密钥</Badge>
                  <button
                    type="button"
                    onClick={() => void copySecret(issuedSecret)}
                    className="inline-flex items-center gap-1 text-[11px] text-xblue hover:underline cursor-pointer"
                  >
                    {copied ? (
                      <>
                        <Check className="h-3 w-3" />
                        已复制
                      </>
                    ) : (
                      <>
                        <Copy className="h-3 w-3" />
                        复制
                      </>
                    )}
                  </button>
                </div>
                <div className="font-mono-tech text-[11px] break-all select-all text-foreground">
                  Authorization: Bearer {issuedSecret}
                </div>
                <p className="text-[10px] text-muted-foreground">
                  密钥已通过服务端密钥环加密落库，关闭后不再回显，请立即保存。
                </p>
              </div>
            )}
          </div>
        </div>
      )}

      <div className="rounded border border-border bg-muted/20 p-2.5 text-[11px] text-muted-foreground leading-relaxed">
        仅部署到 <span className="font-mono-tech text-foreground">PROD</span>{" "}
        环境的已发布版本会响应自动触发（CRON / OUTBOX_EVENT / WEBHOOK / RULE）；手动运行默认执行{" "}
        <span className="font-mono-tech text-foreground">TEST</span> 环境。
      </div>
    </div>
  );
}
