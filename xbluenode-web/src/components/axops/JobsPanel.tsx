import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
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
import {
  Badge,
  Button,
  Card,
  Pagination,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { ALL_JOB_KINDS, ALL_JOB_STATUSES } from "./constants";

export interface JobsPanelProps {
  canManageOps: boolean;
}

const JOBS_PAGE_SIZE = 20;

export function JobsPanel({ canManageOps }: JobsPanelProps) {
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canWrite = canManageOps && can("OPS_ADMIN");

  const [kindFilter, setKindFilter] = useState<JobKind | "">("");
  const [statusFilter, setStatusFilter] = useState<JobStatus | "">("");
  const [jobs, setJobs] = useState<OpsJobRecord[]>([]);
  const [page, setPage] = useState(1);
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
      setPage(1);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("加载任务队列失败", "Failed to load async jobs")
      );
    } finally {
      setLoading(false);
    }
  }, [kindFilter, statusFilter, tx]);

  useEffect(() => {
    void loadJobs();
  }, [loadJobs]);

  const totalPages = Math.max(1, Math.ceil(jobs.length / JOBS_PAGE_SIZE));
  const safePage = Math.min(page, totalPages);
  const pagedJobs = useMemo(
    () =>
      jobs.slice((safePage - 1) * JOBS_PAGE_SIZE, safePage * JOBS_PAGE_SIZE),
    [jobs, safePage]
  );

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
      setNoticeMsg(
        tx(
          `任务 ${jobId.slice(0, 8)} 已重新入队`,
          `Re-enqueued job ${jobId.slice(0, 8)}`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("重试任务失败", "Failed to retry job")
      );
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
      setNoticeMsg(
        tx(
          `任务 ${jobId.slice(0, 8)} 已取消`,
          `Canceled job ${jobId.slice(0, 8)}`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("取消任务失败", "Failed to cancel job")
      );
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
      setNoticeMsg(
        tx(
          `指标汇总任务已入队 (${res.job_id.slice(0, 8)})`,
          `Metric rollup job enqueued (${res.job_id.slice(0, 8)})`
        )
      );
      await loadJobs();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("触发指标汇总失败", "Failed to trigger metric rollup")
      );
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
            <span>{tx("异步任务队列", "Async Job Queue")}</span>
            <Badge variant="outline">
              {tx(`${fmtNum(jobs.length, 0)} 条`, `${fmtNum(jobs.length, 0)} items`)}
            </Badge>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {canWrite && (
              <Button
                type="button"
                variant="primary"
                size="sm"
                data-testid="trigger-rollup-now-btn"
                disabled={rollingUp}
                onClick={() => void handleTriggerRollupNow()}
              >
                <Play className="h-3.5 w-3.5" />
                {rollingUp
                  ? tx("入队中…", "Enqueuing…")
                  : tx("立即汇总指标", "Trigger Rollup")}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="axops-jobs-refresh-btn"
              disabled={loading}
              onClick={() => void loadJobs()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              {tx("刷新", "Refresh")}
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5 text-xs">
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("任务类型", "Job Type")}
            </label>
            <Select
              data-testid="axops-jobs-filter-kind"
              value={kindFilter}
              onChange={(e) => setKindFilter(e.target.value as JobKind | "")}
              className="h-8 w-full text-xs"
            >
              <option value="">{tx("全部类型", "All Types")}</option>
              {ALL_JOB_KINDS.map((k) => (
                <option key={k} value={k}>
                  {tEnum(k)} ({k})
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("状态", "Status")}
            </label>
            <Select
              data-testid="axops-jobs-filter-status"
              value={statusFilter}
              onChange={(e) =>
                setStatusFilter(e.target.value as JobStatus | "")
              }
              className="h-8 w-full text-xs"
            >
              <option value="">{tx("全部状态", "All Statuses")}</option>
              {ALL_JOB_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {tEnum(s)} ({s})
                </option>
              ))}
            </Select>
          </div>

          <div className="flex items-end">
            {(kindFilter || statusFilter) && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  setKindFilter("");
                  setStatusFilter("");
                }}
              >
                {tx("清空筛选", "Clear Filters")}
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

      {/* Jobs Table + Pagination */}
      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3 w-8" />
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("任务类型 / ID", "Job Type / ID")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("状态", "Status")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("尝试次数", "Attempts")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("更新 / 调度时间", "Updated / Next Run")}
                </th>
                <th className="py-2.5 px-3 text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {pagedJobs.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-10 text-center text-muted-foreground"
                    data-testid="axops-jobs-empty"
                  >
                    {loading
                      ? tx("加载中…", "Loading…")
                      : tx("暂无匹配任务", "No matching jobs")}
                  </td>
                </tr>
              ) : (
                pagedJobs.map((job) => {
                  const isExpanded = expandedJobId === job.id;
                  const detail = detailMap[job.id] ?? job;
                  const jobKindCode =
                    (job as OpsJobRecord & { job_kind?: string }).job_kind ??
                    job.kind;
                  const canRetry =
                    canWrite &&
                    (job.status === "FAILED" || job.status === "CANCELED");
                  const canCancel =
                    canWrite &&
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
                          <div
                            className="font-medium text-foreground flex items-center gap-1.5"
                            title={jobKindCode}
                          >
                            <span>{tEnum(jobKindCode)}</span>
                            <span className="font-mono-tech text-[10px] text-muted-foreground">
                              {jobKindCode}
                            </span>
                          </div>
                          <div className="font-mono-tech text-[11px] text-muted-foreground">
                            {job.id.slice(0, 8)}…
                          </div>
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          <StatusBadge status={job.status} />
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech whitespace-nowrap">
                          {job.attempt} / {job.max_attempts}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          <div>{fmtTime(job.updated_at, true)}</div>
                          <div>{fmtTime(job.next_run_at, true)}</div>
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
                                size="sm"
                                data-testid={`retry-job-btn-${job.id}`}
                                disabled={isActing}
                                onClick={() => void handleRetryJob(job.id)}
                              >
                                <RotateCcw className="h-3 w-3 text-xblue" />
                                {tx("重试", "Retry")}
                              </Button>
                            )}
                            {canCancel && (
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                data-testid={`cancel-job-btn-${job.id}`}
                                disabled={isActing}
                                onClick={() => void handleCancelJob(job.id)}
                              >
                                <XCircle className="h-3 w-3" />
                                {tx("取消", "Cancel")}
                              </Button>
                            )}
                          </div>
                        </td>
                      </tr>

                      {isExpanded && (
                        <tr className="bg-muted/20">
                          <td colSpan={6} className="p-3.5 space-y-2">
                            <div className="flex flex-wrap gap-4 text-[11px] font-mono-tech text-muted-foreground">
                              <span>ID: {detail.id}</span>
                              <span>
                                {tx("创建", "Created")}:{" "}
                                {fmtTime(detail.created_at)}
                              </span>
                              <span>
                                {tx("更新", "Updated")}:{" "}
                                {fmtTime(detail.updated_at)}
                              </span>
                            </div>
                            {detail.last_error && (
                              <div className="rounded border border-red-500/40 bg-red-500/10 p-2.5 text-[11px] font-mono-tech text-red-700 dark:text-red-300 break-all">
                                {detail.last_error}
                              </div>
                            )}
                            <div className="space-y-1">
                              <div className="text-[11px] font-medium text-muted-foreground">
                                {tx("执行输出", "Result Payload")}
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
        {jobs.length > JOBS_PAGE_SIZE && (
          <div className="border-t border-border px-4 py-2.5">
            <Pagination
              page={safePage}
              pageSize={JOBS_PAGE_SIZE}
              total={jobs.length}
              onPageChange={setPage}
            />
          </div>
        )}
      </Card>
    </div>
  );
}
