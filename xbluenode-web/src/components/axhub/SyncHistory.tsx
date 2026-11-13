import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  History,
  RefreshCw,
  ShieldCheck,
  XCircle,
} from "lucide-react";
import { axhubApi, type Dataset, type SyncJob } from "@/lib/api/axhub";
import { Badge, Button, Card } from "@/components/ui/primitives";

export interface SyncHistoryProps {
  dataset: Dataset;
  onDatasetUpdated?: () => Promise<void>;
}

export function SyncHistory({ dataset, onDatasetUpdated }: SyncHistoryProps) {
  const [jobs, setJobs] = useState<SyncJob[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [actionBanner, setActionBanner] = useState<string | null>(null);

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
    setErrorMsg(null);
    try {
      await axhubApi.updateDataset(dataset.id, { schema_policy: "EVOLVE" });
      const syncRes = await axhubApi.syncDataset(dataset.id);
      setActionBanner(
        `已切换为 EVOLVE 策略并重新入队同步任务 (${syncRes.job_id})`
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
          <div>
            <h3 className="text-xs font-semibold">
              同步任务历史 · {dataset.name}
            </h3>
            <p className="text-[11px] text-muted-foreground">
              展示 DuckLake 快照提交记录、增删读行数、Schema 演进差异与数据质量校验结果
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="outline"
          className="px-2.5 py-1 text-xs"
          onClick={() => void loadJobs()}
          disabled={loading}
          data-testid="refresh-sync-history-btn"
        >
          <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          刷新历史
        </Button>
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
          {errorMsg}
        </div>
      )}

      {actionBanner && (
        <div className="rounded border border-xblue/30 bg-xblue-soft px-3 py-2 text-xs text-xblue">
          {actionBanner}
        </div>
      )}

      {jobs.length === 0 ? (
        <div className="py-6 text-center text-xs text-muted-foreground">
          {loading ? "正在加载同步记录..." : "当前数据集暂无同步任务历史。"}
        </div>
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
                  <div className="flex items-center gap-2">
                    <Badge
                      variant={
                        job.status === "SUCCEEDED"
                          ? "xblue"
                          : job.status === "FAILED"
                            ? "outline"
                            : "neutral"
                      }
                    >
                      {job.status}
                    </Badge>
                    <Badge variant="neutral">{job.sync_mode}</Badge>
                    {job.snapshot_id !== null && (
                      <span className="font-mono-tech text-muted-foreground">
                        Snapshot #{job.snapshot_id}
                      </span>
                    )}
                    <span className="font-mono-tech text-[11px] text-muted-foreground">
                      Batch: {job.batch_id.slice(0, 8)}
                    </span>
                  </div>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    {job.finished_at ?? job.started_at ?? job.created_at}
                  </span>
                </div>

                <div className="grid grid-cols-2 md:grid-cols-4 gap-2 rounded bg-muted/50 px-2.5 py-1.5 font-mono-tech text-[11px]">
                  <div>读取行: {job.rows_read}</div>
                  <div>写入行: {job.rows_written}</div>
                  <div>删除行: {job.rows_deleted}</div>
                  <div>
                    游标: {job.cursor_from ?? "∅"} → {job.cursor_to ?? "∅"}
                  </div>
                </div>

                {job.error_code && (
                  <div className="rounded border border-foreground/30 bg-muted p-2 flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-1.5">
                      <AlertTriangle className="h-3.5 w-3.5 text-foreground" />
                      <span className="font-mono-tech font-medium">
                        {job.error_code}:
                      </span>
                      <span>{job.error_message ?? "执行异常"}</span>
                    </div>
                    {job.error_code === "SCHEMA_BREAKING_CHANGE" && (
                      <Button
                        type="button"
                        variant="outline"
                        className="px-2.5 py-1 text-[11px]"
                        onClick={() => void handleSwitchEvolveAndRetry()}
                      >
                        改用 EVOLVE 策略后重试
                      </Button>
                    )}
                  </div>
                )}

                {/* Schema Change Summary */}
                <div className="flex flex-wrap items-center gap-2 text-[11px]">
                  <span className="text-muted-foreground font-medium">
                    Schema 变更:
                  </span>
                  {!hasSchemaDiff ? (
                    <span className="text-muted-foreground">无结构变更</span>
                  ) : (
                    <>
                      {addedCount > 0 && (
                        <Badge variant="xblue">
                          +{addedCount} 列 (
                          {job.schema_change.added.map((c) => c.name).join(", ")})
                        </Badge>
                      )}
                      {removedCount > 0 && (
                        <Badge variant="outline">
                          -{removedCount} 列 (
                          {job.schema_change.removed.join(", ")})
                        </Badge>
                      )}
                      {widenedCount > 0 && (
                        <Badge variant="neutral">
                          拓宽:{" "}
                          {job.schema_change.widened
                            .map(([col, fromT, toT]) => `${col}(${fromT}→${toT})`)
                            .join(", ")}
                        </Badge>
                      )}
                      {narrowedCount > 0 && (
                        <Badge variant="outline">
                          收窄:{" "}
                          {job.schema_change.narrowed
                            .map(([col, fromT, toT]) => `${col}(${fromT}→${toT})`)
                            .join(", ")}
                        </Badge>
                      )}
                      {job.schema_change.breaking && (
                        <Badge variant="outline">BREAKING</Badge>
                      )}
                    </>
                  )}
                </div>

                {/* Quality Report Detail */}
                {qualityRules.length > 0 && (
                  <div className="space-y-1 pt-1 border-t border-border">
                    <div className="flex items-center gap-2 text-[11px]">
                      <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
                      <span className="font-medium">质量校验报告:</span>
                      <Badge
                        variant={job.quality_result.passed ? "xblue" : "outline"}
                      >
                        {job.quality_result.passed
                          ? "PASSED"
                          : job.quality_result.blocked
                            ? "BLOCKED"
                            : "WARN"}
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
                              <CheckCircle2 className="h-3 w-3 text-xblue" />
                            ) : (
                              <XCircle className="h-3 w-3 text-muted-foreground" />
                            )}
                            {qr.rule_kind}
                            {qr.column_name ? `(${qr.column_name})` : ""}
                          </span>
                          <span className="text-muted-foreground truncate ml-2">
                            {qr.detail || qr.on_fail}
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
