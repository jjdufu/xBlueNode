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
  X,
} from "lucide-react";
import type { JobStatus } from "@/lib/api/enums";
import { xblueviewApi, type ExportJobRecord } from "@/lib/api/xblueview";
import { Badge, Button } from "@/components/ui/primitives";

export interface ExportListProps {
  open: boolean;
  highlightJobId?: string | null;
  onClose: () => void;
  onShareExportFile: (job: ExportJobRecord) => void;
}

function formatByteSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function formatIsoTime(raw: string | null | undefined): string {
  if (!raw) return "—";
  const d = new Date(raw);
  if (Number.isNaN(d.getTime())) return raw;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours()
  )}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function statusBadgeVariant(status: JobStatus): "xblue" | "neutral" | "outline" {
  if (status === "SUCCEEDED") return "xblue";
  if (status === "PENDING" || status === "RUNNING") return "neutral";
  return "outline";
}

export function ExportList({
  open,
  highlightJobId,
  onClose,
  onShareExportFile,
}: ExportListProps) {
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

  if (!open) return null;

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
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/45"
      data-testid="export-list-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl h-full bg-background text-foreground border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="export-list-drawer"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-4 py-3 border-b border-border flex items-center justify-between gap-2 shrink-0">
          <div className="flex items-center gap-2">
            <FileArchive className="h-4 w-4 text-xblue" />
            <h3 className="text-sm font-semibold">出品中心 · 我的导出任务</h3>
            {hasActiveJobs && (
              <Badge variant="xblue" className="gap-1">
                <Loader2 className="h-3 w-3 animate-spin" />
                自动刷新中
              </Badge>
            )}
          </div>
          <div className="flex items-center gap-1">
            <Button
              type="button"
              variant="ghost"
              onClick={() => void fetchExports(false)}
              disabled={loading}
              className="px-2 py-1 text-xs"
              data-testid="refresh-export-list-btn"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
              />
              刷新
            </Button>
            <button
              type="button"
              onClick={onClose}
              className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
              aria-label="关闭出品中心"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-3 text-xs">
          {errorMsg && (
            <div
              className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
              data-testid="export-list-error"
            >
              {errorMsg}
            </div>
          )}

          {loading && jobs.length === 0 ? (
            <div className="py-16 text-center text-muted-foreground">
              正在加载出品任务记录...
            </div>
          ) : jobs.length === 0 ? (
            <div
              className="rounded-md border border-dashed border-border py-16 text-center text-muted-foreground"
              data-testid="export-list-empty"
            >
              暂无出品记录，点击驾驶舱工具栏「出品导出」生成报告。
            </div>
          ) : (
            jobs.map((job) => {
              const isHighlighted = highlightJobId === job.id;
              const displayTitle =
                job.options?.title ||
                job.file_name ||
                `${job.source.kind} 导出`;
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
                          {job.status}
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
                    <div className="rounded border border-foreground/25 bg-muted px-2.5 py-1.5 text-[11px] flex items-start gap-1.5">
                      <AlertCircle className="h-3.5 w-3.5 shrink-0 mt-0.5" />
                      <span>{job.error_message}</span>
                    </div>
                  )}

                  <div className="flex flex-wrap items-center justify-between gap-2 pt-1.5 border-t border-border/60">
                    <div className="flex flex-wrap items-center gap-3 text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                      <span className="inline-flex items-center gap-1">
                        <Clock className="h-3 w-3" />
                        创建: {formatIsoTime(job.created_at)}
                      </span>
                      {job.expires_at && (
                        <span>到期: {formatIsoTime(job.expires_at)}</span>
                      )}
                    </div>

                    <div className="flex items-center gap-1.5">
                      {job.status === "SUCCEEDED" && (
                        <>
                          <Button
                            type="button"
                            onClick={() => void handleDownload(job)}
                            disabled={downloadingId === job.id}
                            className="h-7 px-2.5 text-xs"
                            data-testid={`download-export-${job.id}`}
                          >
                            <Download className="h-3 w-3" />
                            {downloadingId === job.id ? "下载中..." : "下载"}
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            onClick={() => onShareExportFile(job)}
                            className="h-7 px-2.5 text-xs"
                            title="生成安全外链分享该文件"
                            data-testid={`share-export-file-${job.id}`}
                          >
                            <Share2 className="h-3 w-3 text-xblue" />
                            分享文件
                          </Button>
                        </>
                      )}
                      <button
                        type="button"
                        disabled={deletingId === job.id}
                        onClick={() => void handleDelete(job.id)}
                        className="rounded p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                        title="删除导出记录"
                        data-testid={`delete-export-${job.id}`}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
