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
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { mergeRetentionPoliciesWithDefaults } from "./logic";

export function RetentionPanel() {
  const [policies, setPolicies] = useState<RetentionPolicyRecord[]>([]);
  const [draftDays, setDraftDays] = useState<Record<string, string>>({});
  const [outboxStats, setOutboxStats] = useState<OutboxStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [savingTarget, setSavingTarget] = useState<RetentionTarget | null>(
    null
  );
  const [runningNow, setRunningNow] = useState(false);
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
      setErrorMsg(err instanceof Error ? err.message : "加载保留策略失败");
    } finally {
      setLoading(false);
    }
  }, []);

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
      setErrorMsg("保留天数必须为不小于 1 的正整数");
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
      setNoticeMsg(`已更新 ${target} 保留周期为 ${updated.keep_days} 天`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "保存保留策略失败");
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
      setNoticeMsg(`已立即入队当前租户保留清理任务 (job_id: ${res.job_id})`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "触发保留清理失败");
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
              全生命周期数据保留策略 (11 类目标)
            </div>
            <p className="text-xs text-muted-foreground">
              每小时由全局调度单例按租户执行超期数据回收、过期授权归档与会话清理；支持手动立即触发。
            </p>
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="primary"
              className="h-8 px-3 text-xs"
              data-testid="run-retention-now-btn"
              disabled={runningNow}
              onClick={() => void handleRunRetentionNow()}
            >
              <Play className="h-3.5 w-3.5" />
              {runningNow ? "清理任务入队中..." : "立即运行保留清理"}
            </Button>
            <Button
              type="button"
              variant="outline"
              className="h-8 px-2.5 text-xs"
              disabled={loading}
              onClick={() => void loadRetentionAndOutbox()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              刷新
            </Button>
          </div>
        </Card>

        <Card className="p-4 space-y-1.5" data-testid="retention-outbox-card">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <Layers className="h-3.5 w-3.5 text-xblue" />
              事务外发积压监控
            </span>
            <Badge variant="outline">ax_outbox</Badge>
          </div>
          <div className="flex items-baseline justify-between pt-1">
            <div>
              <span className="text-[11px] text-muted-foreground mr-1.5">
                待消费:
              </span>
              <span className="text-lg font-mono-tech font-semibold">
                {outboxStats ? outboxStats.pending : "—"}
              </span>
            </div>
            <div>
              <span className="text-[11px] text-muted-foreground mr-1.5">
                最大延迟:
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
                <th className="py-2.5 px-3">保留目标 (Target)</th>
                <th className="py-2.5 px-3">回收机制与执行主体说明</th>
                <th className="py-2.5 px-3 w-44">保留天数 (keep_days)</th>
                <th className="py-2.5 px-3 text-right w-28">操作</th>
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
                        {row.label}
                      </div>
                      <div className="flex items-center gap-1.5 mt-1">
                        <Badge variant="outline">{row.target}</Badge>
                        {row.ownerScriptOnly && (
                          <Badge
                            variant="xblue"
                            data-testid="retention-audit-owner-badge"
                          >
                            <ShieldAlert className="h-3 w-3 mr-1" />
                            retention-audit.sh 属主任务
                          </Badge>
                        )}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top text-muted-foreground leading-relaxed max-w-xl">
                      {row.description}
                    </td>

                    <td className="py-3 px-3 align-top">
                      <div className="flex items-center gap-1.5">
                        <Input
                          type="number"
                          min={1}
                          max={3650}
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
                        <span className="text-muted-foreground">天</span>
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top text-right">
                      <Button
                        type="button"
                        variant="outline"
                        className="h-8 px-2.5 text-xs"
                        data-testid={`save-retention-btn-${row.target}`}
                        disabled={isSaving}
                        onClick={() => void handleSaveTarget(row.target)}
                      >
                        <Save className="h-3.5 w-3.5 text-xblue" />
                        {isSaving ? "保存中..." : "保存"}
                      </Button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
