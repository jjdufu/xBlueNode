import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Clock,
  Layers,
  Play,
  RefreshCw,
  Save,
  ShieldAlert,
} from "lucide-react";
import {
  axopsApi,
  type OutboxStats,
  type RetentionPolicyRecord,
} from "@/lib/api/axops";
import type { RetentionTarget } from "@/lib/api/enums";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Input,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { mergeRetentionPoliciesWithDefaults } from "./logic";

export function RetentionPanel() {
  const { isZh, tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canWrite = can("OPS_ADMIN");

  const [policies, setPolicies] = useState<RetentionPolicyRecord[]>([]);
  const [draftDays, setDraftDays] = useState<Record<string, string>>({});
  const [outboxStats, setOutboxStats] = useState<OutboxStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [savingTarget, setSavingTarget] = useState<RetentionTarget | null>(
    null
  );
  const [runningNow, setRunningNow] = useState(false);
  const [confirmRunOpen, setConfirmRunOpen] = useState(false);
  const [noticeMsg, setNoticeMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadRetentionAndOutbox = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const [retRes, outRes] = await Promise.all([
        axopsApi.listRetentionPolicies(),
        axopsApi.getOutboxStats().catch(() => null),
      ]);
      const items = Array.isArray(retRes?.items) ? retRes.items : [];
      setPolicies(items);
      if (outRes) {
        setOutboxStats(outRes);
      }
      const nextDrafts: Record<string, string> = {};
      for (const item of mergeRetentionPoliciesWithDefaults(items)) {
        nextDrafts[item.target] = String(item.keep_days);
      }
      setDraftDays(nextDrafts);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("加载保留策略失败", "Failed to load retention policies")
      );
    } finally {
      setLoading(false);
    }
  }, [tx]);

  useEffect(() => {
    void loadRetentionAndOutbox();
  }, [loadRetentionAndOutbox]);

  const mergedRows = useMemo(
    () => mergeRetentionPoliciesWithDefaults(policies),
    [policies]
  );

  const handleSaveTarget = async (target: RetentionTarget) => {
    const raw = draftDays[target] ?? "";
    const parsed = Number.parseInt(raw, 10);
    if (!Number.isFinite(parsed) || parsed < 1) {
      setErrorMsg(
        tx(
          "保留天数必须为不小于 1 的正整数",
          "Retention days must be an integer ≥ 1"
        )
      );
      return;
    }

    setSavingTarget(target);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const updated = await axopsApi.updateRetentionPolicy(target, parsed);
      setPolicies((prev) => {
        const filtered = prev.filter((p) => p.target !== target);
        return [...filtered, updated];
      });
      setDraftDays((prev) => ({
        ...prev,
        [target]: String(updated.keep_days),
      }));
      setNoticeMsg(
        tx(
          `已保存 ${target} 保留天数为 ${updated.keep_days} 天`,
          `Updated ${target} retention to ${updated.keep_days} days`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("保存保留策略失败", "Failed to save retention policy")
      );
    } finally {
      setSavingTarget(null);
    }
  };

  const handleRunRetentionNow = async () => {
    setRunningNow(true);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const res = await axopsApi.runRetentionNow();
      setConfirmRunOpen(false);
      setNoticeMsg(
        tx(
          `保留清理任务已入队 (${res.job_id.slice(0, 8)})`,
          `Retention cleanup job enqueued (${res.job_id.slice(0, 8)})`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("触发保留清理失败", "Failed to trigger retention cleanup")
      );
    } finally {
      setRunningNow(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="axops-retention-panel">
      {/* Top Action & Outbox Card */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
        <Card className="p-4 lg:col-span-2 flex flex-wrap items-center justify-between gap-3">
          <div className="space-y-1">
            <div className="text-sm font-semibold flex items-center gap-2">
              <Clock className="h-4 w-4 text-xblue" />
              {tx("全生命周期数据保留策略", "Lifecycle Data Retention Policies")}
            </div>
            <p className="text-xs text-muted-foreground">
              {tx(
                "每小时定时执行超期数据回收、过期授权归档与失效会话清理。",
                "Hourly scheduled purge of expired telemetry, logs, grants, and stale sessions."
              )}
            </p>
          </div>

          <div className="flex items-center gap-2">
            {canWrite && (
              <Button
                type="button"
                variant="danger"
                size="sm"
                data-testid="run-retention-now-btn"
                disabled={runningNow}
                onClick={() => setConfirmRunOpen(true)}
              >
                <Play className="h-3.5 w-3.5" />
                {runningNow
                  ? tx("入队中…", "Enqueuing…")
                  : tx("立即清理", "Run Cleanup")}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={loading}
              onClick={() => void loadRetentionAndOutbox()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              {tx("刷新", "Refresh")}
            </Button>
          </div>
        </Card>

        <Card className="p-4 space-y-1.5" data-testid="retention-outbox-card">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <Layers className="h-3.5 w-3.5 text-xblue" />
              {tx("事务外发队列", "Transactional Outbox")}
            </span>
            <Badge variant="outline">ax_outbox</Badge>
          </div>
          <div className="flex items-baseline justify-between pt-1">
            <div>
              <span className="text-[11px] text-muted-foreground mr-1.5">
                {tx("待消费", "Pending")}:
              </span>
              <span className="text-lg font-mono-tech font-semibold">
                {outboxStats ? fmtNum(outboxStats.pending, 0) : "—"}
              </span>
            </div>
            <div>
              <span className="text-[11px] text-muted-foreground mr-1.5">
                {tx("最大延迟", "Max Lag")}:
              </span>
              <span className="text-lg font-mono-tech font-semibold text-xblue">
                {outboxStats ? `${outboxStats.lag_s.toFixed(1)}s` : "—"}
              </span>
            </div>
          </div>
        </Card>
      </div>

      {errorMsg && (
        <div
          role="alert"
          className="rounded-md border border-red-500/40 bg-red-500/10 px-3.5 py-2 text-xs text-red-700 dark:text-red-300"
        >
          {errorMsg}
        </div>
      )}

      {noticeMsg && (
        <div
          data-testid="axops-retention-notice"
          className="rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2 text-xs text-xblue"
        >
          {noticeMsg}
        </div>
      )}

      {/* 11 Retention Targets Table */}
      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("保留目标", "Target")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("回收机制", "Purge Mechanism")}
                </th>
                <th className="py-2.5 px-3 w-44 whitespace-nowrap">
                  {tx("保留天数", "Keep Days")}
                </th>
                <th className="py-2.5 px-3 text-right w-28 whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {mergedRows.map((row) => {
                const draftVal =
                  draftDays[row.target] ?? String(row.keep_days);
                const isSaving = savingTarget === row.target;

                return (
                  <tr
                    key={row.target}
                    data-testid={`retention-row-${row.target}`}
                    className="hover:bg-muted/30 transition-colors"
                  >
                    <td className="py-3 px-3 align-top">
                      <div className="font-medium text-foreground">
                        {isZh ? row.label : row.enLabel}
                      </div>
                      <div className="flex items-center gap-1.5 mt-1">
                        <Badge variant="outline">{row.target}</Badge>
                        {row.ownerScriptOnly && (
                          <Badge
                            variant="xblue"
                            data-testid="retention-audit-owner-badge"
                          >
                            <ShieldAlert className="h-3 w-3 mr-1" />
                            {tx(
                              "属主脚本执行",
                              "Owner Script Only"
                            )}
                          </Badge>
                        )}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top text-muted-foreground leading-relaxed max-w-xl">
                      {isZh ? row.description : row.enDescription}
                    </td>

                    <td className="py-3 px-3 align-top">
                      <div className="flex items-center gap-1.5">
                        <Input
                          type="number"
                          min={1}
                          max={3650}
                          disabled={!canWrite || row.ownerScriptOnly}
                          data-testid={`retention-days-input-${row.target}`}
                          value={draftVal}
                          onChange={(e) =>
                            setDraftDays((prev) => ({
                              ...prev,
                              [row.target]: e.target.value,
                            }))
                          }
                          className="h-8 w-28 text-xs font-mono-tech"
                        />
                        <span className="text-muted-foreground">
                          {tx("天", "days")}
                        </span>
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top text-right">
                      {canWrite && !row.ownerScriptOnly ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          data-testid={`save-retention-btn-${row.target}`}
                          disabled={isSaving}
                          onClick={() => void handleSaveTarget(row.target)}
                        >
                          <Save className="h-3.5 w-3.5 text-xblue" />
                          {isSaving ? tx("保存中…", "Saving…") : tx("保存", "Save")}
                        </Button>
                      ) : row.ownerScriptOnly ? (
                        <span className="text-[11px] text-muted-foreground">
                          {tx("只读受控", "Script locked")}
                        </span>
                      ) : null}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>

      <ConfirmDialog
        open={confirmRunOpen}
        onClose={() => setConfirmRunOpen(false)}
        onConfirm={() => void handleRunRetentionNow()}
        loading={runningNow}
        title={tx("确认执行全量超期数据清理", "Confirm Retention Cleanup")}
        description={tx(
          "将按上方保留天数立即异步清理所有超期作业日志、告警记录、授权与会话，物理删除后不可恢复。",
          "Immediately enqueue permanent purge of expired telemetry, alerts, grants, and sessions across active targets."
        )}
        confirmLabel={tx("确认清理", "Run Cleanup")}
      />
    </div>
  );
}
