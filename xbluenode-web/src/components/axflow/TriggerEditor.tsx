import { useState } from "react";
import { Check, Copy, KeyRound, Webhook } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { axflowApi, type TriggerCfg } from "@/lib/api/axflow";
import type { OutboxTopic, TriggerType } from "@/lib/api/enums";
import { Badge, Button, Input, Select, Switch } from "@/components/ui/primitives";

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
  { zh: "每小时整点", en: "Hourly", expr: "0 */1 * * *" },
  { zh: "每日 02:00", en: "Daily 02:00", expr: "0 2 * * *" },
  { zh: "每 15 分钟", en: "Every 15m", expr: "*/15 * * * *" },
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
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("FLOW_EDIT");

  const [rotating, setRotating] = useState(false);
  const [issuedSecret, setIssuedSecret] = useState<string | null>(null);
  const [hookError, setHookError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const webhookPath = `/api/axflow/hooks/${tenantId || "default"}/${
    slug && slug.trim().length > 0 ? slug.trim() : "<slug>"
  }`;

  const handleRotateSecret = async () => {
    if (!flowId || !canEdit) return;
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
        <label className="block font-medium mb-1">
          {tx("触发方式", "Trigger Mode")}
        </label>
        <Select
          data-testid="trigger-type-select"
          value={triggerCfg.type}
          disabled={!canEdit}
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
          className="h-8 w-full font-mono-tech"
        >
          <option value="MANUAL">
            {tx("手动 / API 触发 (MANUAL)", "Manual / API (MANUAL)")}
          </option>
          <option value="CRON">
            {tx("定时调度 (CRON)", "Scheduled (CRON)")}
          </option>
          <option value="OUTBOX_EVENT">
            {tx("事件总线订阅 (OUTBOX_EVENT)", "Event Bus (OUTBOX_EVENT)")}
          </option>
          <option value="WEBHOOK">
            {tx("外部回调 (WEBHOOK)", "HTTP Webhook (WEBHOOK)")}
          </option>
          <option value="RULE">
            {tx("告警规则联动 (RULE)", "Alert Rule (RULE)")}
          </option>
          <option value="API">
            {tx("数据服务接口 (API)", "Data Service (API)")}
          </option>
          <option value="MCP">
            {tx("智能体工具 (MCP)", "Agent Tool (MCP)")}
          </option>
          <option value="SIM_APPLY">
            {tx("推演决策下发 (SIM_APPLY)", "Sim Decision (SIM_APPLY)")}
          </option>
        </Select>
      </div>

      {triggerCfg.type === "CRON" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">
              {tx("Cron 表达式", "Cron Expression")}
            </label>
            <Input
              data-testid="trigger-cron-input"
              value={triggerCfg.cron ?? ""}
              disabled={!canEdit}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, cron: e.target.value })
              }
              placeholder="0 */1 * * *"
              className="h-8 font-mono-tech text-xs"
            />
            {canEdit && (
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
                    {tx(p.zh, p.en)} ({p.expr})
                  </button>
                ))}
              </div>
            )}
          </div>

          <div>
            <label className="block font-medium mb-1">
              {tx("时区", "Timezone")}
            </label>
            <Input
              data-testid="trigger-timezone-input"
              value={triggerCfg.timezone ?? "Asia/Shanghai"}
              disabled={!canEdit}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, timezone: e.target.value })
              }
              placeholder="Asia/Shanghai"
              className="h-8 font-mono-tech text-xs"
            />
          </div>

          <div className="flex items-center justify-between pt-1">
            <span>{tx("补跑错过的调度周期", "Catch up missed windows")}</span>
            <Switch
              data-testid="trigger-catchup-checkbox"
              checked={Boolean(triggerCfg.catch_up)}
              disabled={!canEdit}
              onCheckedChange={(checked) =>
                onChangeTrigger({ ...triggerCfg, catch_up: checked })
              }
              aria-label={tx("补跑错过的调度周期", "Catch up missed windows")}
            />
          </div>
        </div>
      )}

      {triggerCfg.type === "OUTBOX_EVENT" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">
              {tx("订阅主题", "Event Topic")}
            </label>
            <Select
              data-testid="trigger-topic-select"
              value={triggerCfg.topic ?? "dataset.synced"}
              disabled={!canEdit}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, topic: e.target.value })
              }
              className="h-8 w-full font-mono-tech"
            >
              {OUTBOX_TOPICS.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="block font-medium mb-1">
              {tx("事件载荷过滤", "Payload Filter")}
            </label>
            <Input
              data-testid="trigger-filter-input"
              value={triggerCfg.filter ?? ""}
              disabled={!canEdit}
              onChange={(e) =>
                onChangeTrigger({ ...triggerCfg, filter: e.target.value })
              }
              placeholder="rows_written > 0"
              className="h-8 font-mono-tech text-xs"
            />
          </div>
        </div>
      )}

      {triggerCfg.type === "WEBHOOK" && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/20 p-3">
          <div>
            <label className="block font-medium mb-1">
              {tx("回调标识", "Webhook Slug")}
            </label>
            <Input
              data-testid="trigger-slug-input"
              value={slug ?? ""}
              disabled={!canEdit}
              onChange={(e) => onChangeSlug(e.target.value)}
              placeholder="daily-supply-hook"
              className="h-8 font-mono-tech text-xs"
            />
          </div>

          <div>
            <div className="flex items-center gap-1 text-[11px] font-medium mb-1">
              <Webhook className="h-3 w-3 text-xblue" />
              {tx("回调端点 (POST)", "Endpoint (POST)")}
            </div>
            <div
              data-testid="trigger-webhook-url"
              className="rounded border border-border bg-card px-2.5 py-1.5 font-mono-tech text-[11px] break-all"
            >
              {webhookPath}
            </div>
          </div>

          <div className="pt-1 space-y-2">
            {canEdit && (
              <Button
                type="button"
                variant="outline"
                className="w-full h-8 text-xs"
                data-testid="rotate-webhook-secret-btn"
                disabled={!flowId || rotating}
                onClick={() => void handleRotateSecret()}
              >
                <KeyRound className="h-3.5 w-3.5 text-xblue" />
                {rotating
                  ? tx("轮换中…", "Rotating…")
                  : tx("轮换密钥", "Rotate Secret")}
              </Button>
            )}

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
                  <Badge variant="xblue">
                    {tx("一次性密钥", "One-Time Secret")}
                  </Badge>
                  <button
                    type="button"
                    onClick={() => void copySecret(issuedSecret)}
                    className="inline-flex items-center gap-1 text-[11px] text-xblue hover:underline cursor-pointer"
                  >
                    {copied ? (
                      <>
                        <Check className="h-3 w-3" />
                        {tx("已复制", "Copied")}
                      </>
                    ) : (
                      <>
                        <Copy className="h-3 w-3" />
                        {tx("复制", "Copy")}
                      </>
                    )}
                  </button>
                </div>
                <div className="font-mono-tech text-[11px] break-all select-all text-foreground">
                  Authorization: Bearer {issuedSecret}
                </div>
                <p className="text-[10px] text-muted-foreground">
                  {tx(
                    "密钥加密存储，关闭后不再回显",
                    "Encrypted at rest; shown once"
                  )}
                </p>
              </div>
            )}
          </div>
        </div>
      )}

      <div className="rounded border border-border bg-muted/20 p-2.5 text-[11px] text-muted-foreground leading-relaxed">
        {tx(
          "仅部署至 PROD 环境的版本响应自动触发；手动运行默认使用 TEST 环境。",
          "Automatic triggers execute the deployed PROD version; manual runs default to TEST."
        )}
      </div>
    </div>
  );
}
