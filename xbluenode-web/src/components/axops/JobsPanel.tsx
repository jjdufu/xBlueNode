import { Fragment, useCallback, useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Filter,
  Play,
  RefreshCw,
  RotateCcw,
  XCircle,
} from "lucide-react";
import { axopsApi, type OpsJobRecord } from "@/lib/api/axops";
import type { JobKind, JobStatus } from "@/lib/api/enums";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  ALL_JOB_KINDS,
  ALL_JOB_STATUSES,
  JOB_KIND_LABELS,
  JOB_STATUS_LABELS,
} from "./constants";

export interface JobsPanelProps {
  canManageOps: boolean;
}

function getJobStatusBadgeClass(status: JobStatus): string {
  switch (status) {
    case "SUCCEEDED":
      return "border border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300";
    case "RUNNING":
      return "border border-xblue/40 bg-xblue-soft text-xblue";
    case "PENDING":
      return "border border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300";
    case "FAILED":
      return "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400";
    case "CANCELED":
    case "EXPIRED":
      return "border border-border bg-muted text-muted-foreground";
  }
}

function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

export function JobsPanel({ canManageOps }: JobsPanelProps) {
  const [kindFilter, setKindFilter] = useState<JobKind | "">("");
  const [statusFilter, setStatusFilter] = useState<JobStatus | "">("");
  const [jobs, setJobs] = useState<OpsJobRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [expandedJobId, setExpandedJobId] = useState<string | null>(null);
  const [detailMap, setDetailMap] = useState<Record<string, OpsJobRecord>>({});
  const [actingJobId, setActingJobId] = useState<string | null>(null);
  const [rollingUp, setRollingUp] = useState(false);
  const [noticeMsg, setNoticeMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadJobs = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axopsApi.listJobs({
        kind: kindFilter || undefined,
        status: statusFilter || undefined,
        limit: 100,
      });
      setJobs(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "加载任务列表失败");
    } finally {
      setLoading(false);
    }
  }, [kindFilter, statusFilter]);

  useEffect(() => {
    void loadJobs();
  }, [loadJobs]);

  const handleToggleExpand = async (job: OpsJobRecord) => {
    if (expandedJobId === job.id) {
      setExpandedJobId(null);
      return;
    }
    setExpandedJobId(job.id);
    try {
      const fresh = await axopsApi.getJob(job.id);
      setDetailMap((prev) => ({ ...prev, [job.id]: fresh }));
    } catch {
      setDetailMap((prev) => ({ ...prev, [job.id]: job }));
    }
  };

  const handleRetryJob = async (jobId: string) => {
    setActingJobId(jobId);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const updated = await axopsApi.retryJob(jobId);
      setJobs((prev) => prev.map((j) => (j.id === jobId ? updated : j)));
      setDetailMap((prev) => ({ ...prev, [jobId]: updated }));
      setNoticeMsg(`已重新入队任务 ${jobId.slice(0, 8)} (${updated.status})`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "重试任务失败");
    } finally {
      setActingJobId(null);
    }
  };

  const handleCancelJob = async (jobId: string) => {
    setActingJobId(jobId);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const updated = await axopsApi.cancelJob(jobId);
      setJobs((prev) => prev.map((j) => (j.id === jobId ? updated : j)));
      setDetailMap((prev) => ({ ...prev, [jobId]: updated }));
      setNoticeMsg(`已取消任务 ${jobId.slice(0, 8)} (${updated.status})`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "取消任务失败");
    } finally {
      setActingJobId(null);
    }
  };

  const handleTriggerRollupNow = async () => {
    setRollingUp(true);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const res = await axopsApi.triggerRollupNow();
      setNoticeMsg(`已触发立即执行指标汇总任务 (job_id: ${res.job_id})`);
      await loadJobs();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "触发指标汇总失败");
    } finally {
      setRollingUp(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="axops-jobs-panel">
      {/* Toolbar */}
      <Card className="p-3.5 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2 text-xs font-medium">
            <Filter className="h-3.5 w-3.5 text-xblue" />
            <span>后台异步任务队列 (ax_jobs)</span>
            <Badge variant="outline">{jobs.length} 条</Badge>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {canManageOps && (
              <Button
                type="button"
                variant="primary"
                className="h-8 px-3 text-xs"
                data-testid="trigger-rollup-now-btn"
                disabled={rollingUp}
                onClick={() => void handleTriggerRollupNow()}
              >
                <Play className="h-3.5 w-3.5" />
                {rollingUp ? "入队中..." : "立即执行指标汇总"}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              className="h-8 px-2.5 text-xs"
              data-testid="axops-jobs-refresh-btn"
              disabled={loading}
              onClick={() => void loadJobs()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              刷新
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5 text-xs">
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              任务类型 (Kind)
            </label>
            <select
              data-testid="axops-jobs-filter-kind"
              value={kindFilter}
              onChange={(e) => setKindFilter(e.target.value as JobKind | "")}
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
            >
              <option value="">全部任务类型</option>
              {ALL_JOB_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k} · {JOB_KIND_LABELS[k]}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              任务状态 (Status)
            </label>
            <select
              data-testid="axops-jobs-filter-status"
              value={statusFilter}
              onChange={(e) =>
                setStatusFilter(e.target.value as JobStatus | "")
              }
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
            >
              <option value="">全部状态</option>
              {ALL_JOB_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {s} · {JOB_STATUS_LABELS[s]}
                </option>
              ))}
            </select>
          </div>

          <div className="flex items-end">
            {(kindFilter || statusFilter) && (
              <Button
                type="button"
                variant="ghost"
                className="h-8 px-2.5 text-xs"
                onClick={() => {
                  setKindFilter("");
                  setStatusFilter("");
                }}
              >
                重置筛选条件
              </Button>
            )}
          </div>
        </div>
      </Card>

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
          data-testid="axops-jobs-notice"
          className="rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2 text-xs text-xblue"
        >
          {noticeMsg}
        </div>
      )}

      {/* Jobs Table */}
      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3 w-8" />
                <th className="py-2.5 px-3">任务 ID / 类型</th>
                <th className="py-2.5 px-3">状态</th>
                <th className="py-2.5 px-3">尝试次数</th>
                <th className="py-2.5 px-3">下次执行 / 更新时间</th>
                <th className="py-2.5 px-3 text-right">管控操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {jobs.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-10 text-center text-muted-foreground"
                    data-testid="axops-jobs-empty"
                  >
                    {loading ? "正在加载后台任务..." : "当前筛选条件下暂无后台任务"}
                  </td>
                </tr>
              ) : (
                jobs.map((job) => {
                  const isExpanded = expandedJobId === job.id;
                  const detail = detailMap[job.id] ?? job;
                  const canRetry =
                    canManageOps &&
                    (job.status === "FAILED" || job.status === "CANCELED");
                  const canCancel =
                    canManageOps &&
                    (job.status === "PENDING" || job.status === "RUNNING");
                  const isActing = actingJobId === job.id;

                  return (
                    <Fragment key={job.id}>
                      <tr
                        data-testid={`ops-job-row-${job.id}`}
                        className="hover:bg-muted/30 transition-colors cursor-pointer"
                        onClick={() => void handleToggleExpand(job)}
                      >
                        <td className="py-2.5 px-3 text-muted-foreground">
                          {isExpanded ? (
                            <ChevronDown className="h-3.5 w-3.5" />
                          ) : (
                            <ChevronRight className="h-3.5 w-3.5" />
                          )}
                        </td>
                        <td className="py-2.5 px-3">
                          <div className="font-medium text-foreground">
                            {JOB_KIND_LABELS[job.kind] ?? job.kind}{" "}
                            <span className="font-mono-tech text-[11px] text-muted-foreground">
                              ({job.kind})
                            </span>
                          </div>
                          <div className="font-mono-tech text-[11px] text-muted-foreground">
                            {job.id}
                          </div>
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          <span
                            className={cn(
                              "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                              getJobStatusBadgeClass(job.status)
                            )}
                          >
                            {job.status}
                          </span>
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech">
                          {job.attempt} / {job.max_attempts}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          <div>更新: {formatDateTime(job.updated_at)}</div>
                          <div>调度: {formatDateTime(job.next_run_at)}</div>
                        </td>
                        <td
                          className="py-2.5 px-3 text-right whitespace-nowrap"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <div className="inline-flex items-center gap-1.5">
                            {canRetry && (
                              <Button
                                type="button"
                                variant="outline"
                                className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                                data-testid={`retry-job-btn-${job.id}`}
                                disabled={isActing}
                                onClick={() => void handleRetryJob(job.id)}
                              >
                                <RotateCcw className="h-3 w-3" />
                                重试
                              </Button>
                            )}
                            {canCancel && (
                              <Button
                                type="button"
                                variant="outline"
                                className="h-7 px-2.5 text-[11px]"
                                data-testid={`cancel-job-btn-${job.id}`}
                                disabled={isActing}
                                onClick={() => void handleCancelJob(job.id)}
                              >
                                <XCircle className="h-3 w-3" />
                                取消
                              </Button>
                            )}
                          </div>
                        </td>
                      </tr>

                      {isExpanded && (
                        <tr className="bg-muted/20">
                          <td colSpan={6} className="p-3.5 space-y-2">
                            <div className="flex flex-wrap gap-4 text-[11px] font-mono-tech text-muted-foreground">
                              <span>创建时间: {formatDateTime(detail.created_at)}</span>
                              <span>更新时间: {formatDateTime(detail.updated_at)}</span>
                              <span>下次运行: {formatDateTime(detail.next_run_at)}</span>
                            </div>
                            {detail.last_error && (
                              <div className="rounded border border-red-500/40 bg-red-500/10 p-2.5 text-[11px] font-mono-tech text-red-700 dark:text-red-300 break-all">
                                最近错误 (last_error): {detail.last_error}
                              </div>
                            )}
                            <div className="space-y-1">
                              <div className="text-[11px] font-medium text-muted-foreground">
                                执行结果快照 (result):
                              </div>
                              <pre className="rounded border border-border bg-background p-2.5 text-[11px] font-mono-tech overflow-x-auto max-h-48">
                                {detail.result
                                  ? JSON.stringify(detail.result, null, 2)
                                  : "{}"}
                              </pre>
                            </div>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
