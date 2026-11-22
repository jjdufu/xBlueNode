import { useCallback, useEffect, useState } from "react";
import {
  AlertCircle,
  Clock,
  Download,
  FileArchive,
  Loader2,
  RefreshCw,
  Share2,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { JobStatus } from "@/lib/api/enums";
import { xblueviewApi, type ExportJobRecord } from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { Badge, Button, Drawer, IconButton } from "@/components/ui/primitives";

export interface ExportListProps {
  open: boolean;
  highlightJobId?: string | null;
  onClose: () => void;
  onShareExportFile: (job: ExportJobRecord) => void;
}

function formatByteSize(bytes: number | null | undefined): string {
  if (
    bytes === null ||
    bytes === undefined ||
    !Number.isFinite(bytes) ||
    bytes < 0
  ) {
    return "—";
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function statusBadgeVariant(
  status: JobStatus
): "success" | "xblue" | "danger" | "outline" {
  if (status === "SUCCEEDED") return "success";
  if (status === "PENDING" || status === "RUNNING") return "xblue";
  if (status === "FAILED") return "danger";
  return "outline";
}

export function ExportList({
  open,
  highlightJobId,
  onClose,
  onShareExportFile,
}: ExportListProps) {
  const { tx, tEnum, fmtTime, isZh } = useI18n();
  const { can } = usePrincipal();
  const canShare = can("VIEW_SHARE");

  const [jobs, setJobs] = useState<ExportJobRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [downloadingId, setDownloadingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const fetchExports = useCallback(async (silent = false) => {
    if (!silent) {
      setLoading(true);
    }
    try {
      const res = await xblueviewApi.listExports({ mine: true, limit: 50 });
      setJobs(Array.isArray(res?.items) ? res.items : []);
      if (!silent) {
        setErrorMsg(null);
      }
    } catch (err) {
      if (!silent) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (!silent) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    void fetchExports(false);
  }, [fetchExports, highlightJobId, open]);

  const hasActiveJobs = jobs.some(
    (j) => j.status === "PENDING" || j.status === "RUNNING"
  );

  useEffect(() => {
    if (!open || !hasActiveJobs) return;
    const timer = setInterval(() => {
      void fetchExports(true);
    }, 2000);
    return () => clearInterval(timer);
  }, [fetchExports, hasActiveJobs, open]);

  const handleDownload = async (job: ExportJobRecord) => {
    setDownloadingId(job.id);
    setErrorMsg(null);
    try {
      await xblueviewApi.downloadExport(job.id, job.file_name ?? undefined);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDownloadingId(null);
    }
  };

  const handleDelete = async (jobId: string) => {
    setDeletingId(jobId);
    setErrorMsg(null);
    try {
      await xblueviewApi.deleteExport(jobId);
      setJobs((prev) => prev.filter((item) => item.id !== jobId));
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingId(null);
    }
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      testId="export-list-drawer"
      closeAriaLabel={tx("关闭出品中心", "Close Export Center")}
      widthClass="max-w-xl"
      title={
        <>
          <FileArchive className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("出品中心", "Export Center")}</span>
          {hasActiveJobs && (
            <Badge variant="xblue" className="gap-1">
              <Loader2 className="h-3 w-3 animate-spin" />
              {tx("同步中", "Syncing")}
            </Badge>
          )}
        </>
      }
    >
      <div className="flex items-center justify-between gap-2 pb-2 border-b border-border">
        <span className="text-muted-foreground">
          {tx(`共 ${jobs.length} 条任务`, `${jobs.length} export jobs`)}
        </span>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void fetchExports(false)}
          disabled={loading}
          data-testid="refresh-export-list-btn"
        >
          <RefreshCw
            className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
          />
          {tx("刷新", "Refresh")}
        </Button>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300"
          data-testid="export-list-error"
        >
          {errorMsg}
        </div>
      )}

      {loading && jobs.length === 0 ? (
        <div className="py-16 text-center text-muted-foreground">
          {tx("加载中…", "Loading…")}
        </div>
      ) : jobs.length === 0 ? (
        <div
          className="rounded-md border border-dashed border-border py-14 text-center text-muted-foreground"
          data-testid="export-list-empty"
        >
          {tx("暂无出品任务记录", "No export jobs")}
        </div>
      ) : (
        <div className="space-y-2.5">
          {jobs.map((job) => {
            const isHighlighted = highlightJobId === job.id;
            const displayTitle =
              job.options?.title ||
              job.file_name ||
              `${tEnum(job.source.kind)} ${tx("导出", "Export")}`;
            const statusText = isZh
              ? `${tEnum(job.status)} · ${job.status}`
              : job.status;
            return (
              <div
                key={job.id}
                data-testid={`export-job-row-${job.id}`}
                className={`rounded-md border p-3 space-y-2 transition-colors ${
                  isHighlighted
                    ? "border-xblue bg-xblue-soft/30"
                    : "border-border bg-card"
                }`}
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0 space-y-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <Badge variant="outline">{job.export_format}</Badge>
                      <Badge
                        variant={statusBadgeVariant(job.status)}
                        data-testid={`export-job-status-${job.id}`}
                      >
                        {(job.status === "PENDING" ||
                          job.status === "RUNNING") && (
                          <Loader2 className="h-2.5 w-2.5 mr-1 animate-spin" />
                        )}
                        {statusText}
                      </Badge>
                      <span className="font-semibold text-foreground truncate">
                        {displayTitle}
                      </span>
                    </div>
                    {job.file_name && (
                      <div className="font-mono-tech text-[11px] text-muted-foreground truncate">
                        {job.file_name}
                      </div>
                    )}
                  </div>

                  <span className="font-mono-tech text-[11px] tabular-nums text-muted-foreground shrink-0">
                    {formatByteSize(job.file_size)}
                  </span>
                </div>

                {job.status === "FAILED" && job.error_message && (
                  <div className="rounded border border-red-500/35 bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-700 dark:text-red-300 flex items-start gap-1.5">
                    <AlertCircle className="h-3.5 w-3.5 shrink-0 mt-0.5" />
                    <span>{job.error_message}</span>
                  </div>
                )}

                <div className="flex flex-wrap items-center justify-between gap-2 pt-1.5 border-t border-border/60">
                  <div className="flex flex-wrap items-center gap-3 text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                    <span className="inline-flex items-center gap-1">
                      <Clock className="h-3 w-3" />
                      {fmtTime(job.created_at, true)}
                    </span>
                    {job.expires_at && (
                      <span>
                        {tx("到期", "Exp")}: {fmtTime(job.expires_at, false)}
                      </span>
                    )}
                  </div>

                  <div className="flex items-center gap-1.5">
                    {job.status === "SUCCEEDED" && (
                      <>
                        <Button
                          type="button"
                          size="sm"
                          onClick={() => void handleDownload(job)}
                          disabled={downloadingId === job.id}
                          data-testid={`download-export-${job.id}`}
                        >
                          <Download className="h-3 w-3" />
                          {downloadingId === job.id
                            ? tx("下载中…", "Downloading…")
                            : tx("下载", "Download")}
                        </Button>
                        {canShare && (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => onShareExportFile(job)}
                            title={tx("分享文件", "Share file")}
                            data-testid={`share-export-file-${job.id}`}
                          >
                            <Share2 className="h-3 w-3 text-xblue" />
                            {tx("分享", "Share")}
                          </Button>
                        )}
                      </>
                    )}
                    <IconButton
                      size="sm"
                      disabled={deletingId === job.id}
                      onClick={() => void handleDelete(job.id)}
                      aria-label={tx("删除记录", "Delete record")}
                      title={tx("删除记录", "Delete record")}
                      data-testid={`delete-export-${job.id}`}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </IconButton>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </Drawer>
  );
}
