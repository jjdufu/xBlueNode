import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  History,
  RefreshCw,
  ShieldCheck,
  XCircle,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { axhubApi, type Dataset, type SyncJob } from "@/lib/api/axhub";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  StatusBadge,
} from "@/components/ui/primitives";

export interface SyncHistoryProps {
  dataset: Dataset;
  onDatasetUpdated?: () => Promise<void>;
}

export function SyncHistory({ dataset, onDatasetUpdated }: SyncHistoryProps) {
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canSync = can("DATA_SYNC");

  const [jobs, setJobs] = useState<SyncJob[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [actionBanner, setActionBanner] = useState<string | null>(null);

  const formatRuleKind = (kind: string): string => {
    switch (kind) {
      case "NOT_NULL":
        return tx("非空校验", "Not Null");
      case "UNIQUE":
        return tx("唯一性校验", "Unique");
      case "RANGE":
        return tx("数值区间", "Range");
      case "REGEX":
        return tx("正则匹配", "Regex");
      case "ALLOWED_VALUES":
        return tx("枚举集合", "Allowed Values");
      case "FRESHNESS":
        return tx("时效性", "Freshness");
      case "ROW_COUNT_MIN":
        return tx("最小行数", "Min Rows");
      default:
        return kind;
    }
  };

  const loadJobs = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axhubApi.listSyncJobs(dataset.id, 20);
      setJobs(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [dataset.id]);

  useEffect(() => {
    void loadJobs();
  }, [loadJobs]);

  const handleSwitchEvolveAndRetry = async () => {
    if (!canSync) return;
    setErrorMsg(null);
    try {
      await axhubApi.updateDataset(dataset.id, { schema_policy: "EVOLVE" });
      const syncRes = await axhubApi.syncDataset(dataset.id);
      setActionBanner(
        tx(
          `演进同步任务已入队 (${syncRes.job_id.slice(0, 8)})`,
          `Schema evolution sync queued (${syncRes.job_id.slice(0, 8)})`
        )
      );
      if (onDatasetUpdated) {
        await onDatasetUpdated();
      }
      await loadJobs();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <Card className="p-4 space-y-3" data-testid="sync-history-panel">
      <div className="flex items-center justify-between border-b border-border pb-2.5">
        <div className="flex items-center gap-2">
          <History className="h-4 w-4 text-xblue" />
          <h3 className="text-xs font-semibold">
            {tx("同步记录", "Sync History")} · {dataset.name}
          </h3>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void loadJobs()}
          disabled={loading}
          data-testid="refresh-sync-history-btn"
        >
          <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          {tx("刷新", "Refresh")}
        </Button>
      </div>

      {errorMsg && (
        <div className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-600 dark:text-red-400">
          {errorMsg}
        </div>
      )}

      {actionBanner && (
        <div className="rounded border border-xblue/30 bg-xblue-soft px-3 py-2 text-xs text-xblue">
          {actionBanner}
        </div>
      )}

      {jobs.length === 0 ? (
        <EmptyState
          title={
            loading
              ? tx("加载中…", "Loading…")
              : tx("暂无同步记录", "No sync jobs recorded")
          }
        />
      ) : (
        <div className="space-y-2.5" data-testid="sync-job-list">
          {jobs.map((job) => {
            const addedCount = job.schema_change?.added?.length ?? 0;
            const removedCount = job.schema_change?.removed?.length ?? 0;
            const widenedCount = job.schema_change?.widened?.length ?? 0;
            const narrowedCount = job.schema_change?.narrowed?.length ?? 0;
            const hasSchemaDiff =
              addedCount + removedCount + widenedCount + narrowedCount > 0 ||
              Boolean(job.schema_change?.breaking);

            const qualityRules = Array.isArray(job.quality_result?.rules)
              ? job.quality_result.rules
              : [];

            return (
              <div
                key={job.id}
                className="rounded border border-border p-3 text-xs space-y-2 bg-background"
                data-testid={`sync-job-item-${job.id}`}
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <StatusBadge status={job.status} />
                    <Badge variant="neutral">{tEnum(job.sync_mode)}</Badge>
                    {job.snapshot_id !== null && (
                      <span className="font-mono-tech text-muted-foreground">
                        #{fmtNum(job.snapshot_id, 0)}
                      </span>
                    )}
                    <span className="font-mono-tech text-[11px] text-muted-foreground">
                      {job.batch_id.slice(0, 8)}
                    </span>
                  </div>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    {fmtTime(job.finished_at ?? job.started_at ?? job.created_at)}
                  </span>
                </div>

                <div className="grid grid-cols-2 md:grid-cols-4 gap-2 rounded bg-muted/50 px-2.5 py-1.5 font-mono-tech text-[11px]">
                  <div>
                    {tx("读取", "Read")}: {fmtNum(job.rows_read, 0)}
                  </div>
                  <div>
                    {tx("写入", "Written")}: {fmtNum(job.rows_written, 0)}
                  </div>
                  <div>
                    {tx("删除", "Deleted")}: {fmtNum(job.rows_deleted, 0)}
                  </div>
                  <div className="truncate" title={`${job.cursor_from ?? "-"} -> ${job.cursor_to ?? "-"}`}>
                    {tx("游标", "Cursor")}: {job.cursor_from ? fmtTime(job.cursor_from, true) : "∅"} →{" "}
                    {job.cursor_to ? fmtTime(job.cursor_to, true) : "∅"}
                  </div>
                </div>

                {job.error_code && (
                  <div className="rounded border border-red-500/30 bg-red-500/5 p-2 flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-1.5">
                      <AlertTriangle className="h-3.5 w-3.5 text-red-500" />
                      <span className="font-mono-tech font-medium">
                        {job.error_code}:
                      </span>
                      <span>
                        {job.error_message ?? tx("执行异常", "Execution failed")}
                      </span>
                    </div>
                    {job.error_code === "SCHEMA_BREAKING_CHANGE" && (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={!canSync}
                        onClick={() => void handleSwitchEvolveAndRetry()}
                      >
                        {tx("演进重试", "Retry Evolve")}
                      </Button>
                    )}
                  </div>
                )}

                {/* Schema Change Summary */}
                <div className="flex flex-wrap items-center gap-2 text-[11px]">
                  <span className="text-muted-foreground font-medium">
                    {tx("结构变更", "Schema Diff")}:
                  </span>
                  {!hasSchemaDiff ? (
                    <span className="text-muted-foreground">
                      {tx("无变更", "None")}
                    </span>
                  ) : (
                    <>
                      {addedCount > 0 && (
                        <Badge variant="xblue">
                          +{fmtNum(addedCount, 0)} (
                          {job.schema_change.added.map((c) => c.name).join(", ")})
                        </Badge>
                      )}
                      {removedCount > 0 && (
                        <Badge variant="outline">
                          -{fmtNum(removedCount, 0)} (
                          {job.schema_change.removed.join(", ")})
                        </Badge>
                      )}
                      {widenedCount > 0 && (
                        <Badge variant="neutral">
                          {tx("类型拓宽", "Widened")}:{" "}
                          {job.schema_change.widened
                            .map(([col, fromT, toT]) => `${col}(${fromT}→${toT})`)
                            .join(", ")}
                        </Badge>
                      )}
                      {narrowedCount > 0 && (
                        <Badge variant="warning">
                          {tx("类型收窄", "Narrowed")}:{" "}
                          {job.schema_change.narrowed
                            .map(([col, fromT, toT]) => `${col}(${fromT}→${toT})`)
                            .join(", ")}
                        </Badge>
                      )}
                      {job.schema_change.breaking && (
                        <Badge variant="danger">
                          {tx("不兼容变更", "Breaking")}
                        </Badge>
                      )}
                    </>
                  )}
                </div>

                {/* Quality Report Detail */}
                {qualityRules.length > 0 && (
                  <div className="space-y-1 pt-1 border-t border-border">
                    <div className="flex items-center gap-2 text-[11px]">
                      <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
                      <span className="font-medium">
                        {tx("质量校验", "Quality Check")}:
                      </span>
                      <Badge
                        variant={
                          job.quality_result.passed
                            ? "success"
                            : job.quality_result.blocked
                              ? "danger"
                              : "warning"
                        }
                      >
                        {job.quality_result.passed
                          ? tx("通过", "Passed")
                          : job.quality_result.blocked
                            ? tEnum("BLOCKED")
                            : tEnum("WARN")}
                      </Badge>
                    </div>
                    <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5">
                      {qualityRules.map((qr) => (
                        <div
                          key={qr.rule_id}
                          className="rounded border border-border px-2 py-1 text-[11px] flex items-center justify-between"
                        >
                          <span className="flex items-center gap-1 font-mono-tech">
                            {qr.passed ? (
                              <CheckCircle2 className="h-3 w-3 text-emerald-600 dark:text-emerald-400" />
                            ) : (
                              <XCircle className="h-3 w-3 text-red-500" />
                            )}
                            <span>{formatRuleKind(qr.rule_kind)}</span>
                            {qr.column_name ? `(${qr.column_name})` : ""}
                          </span>
                          <span className="text-muted-foreground truncate ml-2">
                            {qr.detail ||
                              (qr.on_fail === "BLOCK"
                                ? tx("阻断", "Block")
                                : tEnum("WARN"))}
                          </span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}
