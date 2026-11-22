import { useEffect, useRef, useState } from "react";
import {
  Database,
  FileText,
  FileUp,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { axhubApi, type UploadRecord } from "@/lib/api/axhub";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";

export interface UploadDropzoneProps {
  onCommitted?: () => Promise<void>;
  onSelectDatasetId?: (datasetId: string) => void;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

export function UploadDropzone({
  onCommitted,
  onSelectDatasetId,
}: UploadDropzoneProps) {
  const { tx, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canSync = can("DATA_SYNC");

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const dragDepthRef = useRef(0);

  const [dragging, setDragging] = useState(false);
  const [toKnowledge, setToKnowledge] = useState(true);
  const [toDatasets, setToDatasets] = useState(true);
  const [datasetPrefix, setDatasetPrefix] = useState("");
  const [jsonExplode, setJsonExplode] = useState(true);
  const [ocrEnabled, setOcrEnabled] = useState(false);
  const [logFormat, setLogFormat] = useState("");

  const [uploads, setUploads] = useState<UploadRecord[]>([]);
  const [dedupFlags, setDedupFlags] = useState<Record<string, boolean>>({});
  const [uploading, setUploading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const handleFiles = async (fileList: FileList | File[]) => {
    if (!canSync) return;
    const files = Array.from(fileList);
    if (files.length === 0) return;

    setUploading(true);
    setErrorMsg(null);

    try {
      for (const file of files) {
        const form = new FormData();
        form.append("file", file);
        form.append("to_knowledge", String(toKnowledge));
        form.append("to_datasets", String(toDatasets));
        if (datasetPrefix.trim()) {
          form.append("dataset_prefix", datasetPrefix.trim());
        }
        form.append("json_explode", String(jsonExplode));
        form.append("ocr", String(ocrEnabled));
        if (logFormat) {
          form.append("log_format", logFormat);
        }

        const enqueued = await axhubApi.uploadFile(form);
        if (enqueued.deduplicated) {
          setDedupFlags((prev) => ({ ...prev, [enqueued.upload_id]: true }));
        }

        const initialRecord = await axhubApi.getUpload(enqueued.upload_id);
        setUploads((prev) => {
          const filtered = prev.filter((item) => item.id !== initialRecord.id);
          return [initialRecord, ...filtered];
        });

        if (initialRecord.status === "COMMITTED" && onCommitted) {
          await onCommitted();
        }
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUploading(false);
    }
  };

  useEffect(() => {
    const activeIds = uploads
      .filter((u) => u.status === "PENDING" || u.status === "PARSED")
      .map((u) => u.id);

    if (activeIds.length === 0) {
      return;
    }

    let cancelled = false;
    const timer = window.setInterval(() => {
      void (async () => {
        let newlyCommitted = false;
        const updatedMap = new Map<string, UploadRecord>();

        for (const id of activeIds) {
          try {
            const latest = await axhubApi.getUpload(id);
            updatedMap.set(id, latest);
            if (latest.status === "COMMITTED") {
              newlyCommitted = true;
            }
          } catch {
            // Keep polling until terminal state
          }
        }

        if (cancelled || updatedMap.size === 0) return;

        setUploads((prev) =>
          prev.map((item) => updatedMap.get(item.id) ?? item)
        );

        if (newlyCommitted && onCommitted) {
          await onCommitted();
        }
      })();
    }, 1200);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [uploads, onCommitted]);

  const refreshSingle = async (id: string) => {
    try {
      const latest = await axhubApi.getUpload(id);
      setUploads((prev) =>
        prev.map((item) => (item.id === id ? latest : item))
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const renderUploadStatus = (rec: UploadRecord) => {
    if (rec.status === "PARSED") {
      return (
        <Badge variant="xblue" className="gap-1">
          <Loader2 className="h-3 w-3 animate-spin" />
          {tx("已解析", "Parsed")}
        </Badge>
      );
    }
    return <StatusBadge status={rec.status} />;
  };

  return (
    <Card className="p-4 space-y-4" data-testid="upload-dropzone-panel">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
        <h2 className="text-sm font-semibold">
          {tx("双轨文件解析入湖", "Dual-Track File Ingestion")}
        </h2>
        <Badge variant="outline">
          CSV · XLSX · DOCX · PDF · JSON · LOG
        </Badge>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs flex items-center justify-between"
          data-testid="upload-error-banner"
        >
          <span className="text-red-600 dark:text-red-400">{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            {tx("关闭", "Dismiss")}
          </button>
        </div>
      )}

      {/* Upload Options */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-3 text-xs">
        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("数据集前缀", "Dataset Prefix")}
          </label>
          <Input
            value={datasetPrefix}
            disabled={!canSync}
            onChange={(e) => setDatasetPrefix(e.target.value)}
            placeholder="supply_chain"
            data-testid="upload-prefix-input"
          />
        </div>

        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("日志格式", "Log Format")}
          </label>
          <Select
            value={logFormat}
            disabled={!canSync}
            onChange={(e) => setLogFormat(e.target.value)}
            className="w-full"
          >
            <option value="">{tx("自动识别", "Auto-detect")}</option>
            <option value="nginx">Nginx Combined</option>
            <option value="log4j">Log4j</option>
            <option value="syslog">RFC 3164 Syslog</option>
            <option value="json">JSON Lines</option>
          </Select>
        </div>

        <div className="flex flex-col justify-center gap-1.5 pt-2">
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={toDatasets}
              disabled={!canSync}
              onChange={(e) => setToDatasets(e.target.checked)}
              data-testid="upload-to-datasets"
            />
            <span>{tx("写入结构化数据集", "Create Lake Datasets")}</span>
          </label>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={toKnowledge}
              disabled={!canSync}
              onChange={(e) => setToKnowledge(e.target.checked)}
              data-testid="upload-to-knowledge"
            />
            <span>{tx("写入超图知识库", "Ingest Knowledge Graph")}</span>
          </label>
        </div>

        <div className="flex flex-col justify-center gap-1.5 pt-2">
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={jsonExplode}
              disabled={!canSync}
              onChange={(e) => setJsonExplode(e.target.checked)}
            />
            <span>{tx("嵌套数组拆分子表", "Explode Nested Arrays")}</span>
          </label>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={ocrEnabled}
              disabled={!canSync}
              onChange={(e) => setOcrEnabled(e.target.checked)}
            />
            <span>{tx("启用光学字符识别", "Enable OCR")}</span>
          </label>
        </div>
      </div>

      {/* Dropzone */}
      <div
        onDragEnter={(e) => {
          e.preventDefault();
          if (!canSync) return;
          dragDepthRef.current += 1;
          setDragging(true);
        }}
        onDragOver={(e) => {
          e.preventDefault();
          if (!canSync) return;
          e.dataTransfer.dropEffect = "copy";
        }}
        onDragLeave={(e) => {
          e.preventDefault();
          dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
          if (
            dragDepthRef.current === 0 ||
            !e.currentTarget.contains(e.relatedTarget as Node | null)
          ) {
            dragDepthRef.current = 0;
            setDragging(false);
          }
        }}
        onDrop={(e) => {
          e.preventDefault();
          dragDepthRef.current = 0;
          setDragging(false);
          if (!canSync) return;
          if (e.dataTransfer.files.length > 0) {
            void handleFiles(e.dataTransfer.files);
          }
        }}
        onClick={() => {
          if (canSync) fileInputRef.current?.click();
        }}
        className={`rounded-md border-2 border-dashed p-8 text-center transition-colors ${
          !canSync
            ? "border-border bg-muted/20 opacity-60 cursor-not-allowed"
            : dragging
              ? "border-xblue bg-xblue-soft/30 cursor-copy"
              : "border-border hover:border-foreground/40 bg-muted/20 cursor-pointer"
        }`}
        data-testid="upload-dropzone"
      >
        <input
          ref={fileInputRef}
          type="file"
          multiple
          disabled={!canSync}
          className="hidden"
          onChange={(e) => {
            if (e.target.files) {
              void handleFiles(e.target.files);
              e.target.value = "";
            }
          }}
          data-testid="upload-file-input"
        />
        <FileUp className="h-6 w-6 mx-auto text-xblue mb-2" />
        <div className="text-xs font-medium">
          {uploading
            ? tx("上传解析中…", "Uploading & parsing…")
            : tx("拖拽文件至此，或浏览本地文件", "Drop files here or browse")}
        </div>
        <div className="text-[11px] text-muted-foreground mt-1">
          {tx("SHA-256 哈希自动幂等去重", "Automatic SHA-256 deduplication")}
        </div>
      </div>

      {/* Upload Records */}
      {uploads.length > 0 && (
        <div className="space-y-2" data-testid="upload-records-list">
          <div className="text-xs font-medium">
            {tx("入湖任务记录", "Upload Tasks")} ({fmtNum(uploads.length, 0)})
          </div>
          {uploads.map((rec) => (
            <div
              key={rec.id}
              className="rounded border border-border p-3 text-xs space-y-2 bg-background"
              data-testid={`upload-record-${rec.filename}`}
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{rec.filename}</span>
                  <span className="font-mono-tech text-muted-foreground">
                    {formatBytes(rec.size_bytes)} · {rec.sha256.slice(0, 10)}
                  </span>
                  {dedupFlags[rec.id] && (
                    <Badge variant="outline">
                      {tx("幂等复用", "Deduplicated")}
                    </Badge>
                  )}
                </div>

                <div className="flex items-center gap-2">
                  {renderUploadStatus(rec)}
                  {rec.error_code && (
                    <Badge variant="danger">{rec.error_code}</Badge>
                  )}
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    {fmtTime(rec.created_at, true)}
                  </span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => void refreshSingle(rec.id)}
                    aria-label={tx("刷新", "Refresh")}
                  >
                    <RefreshCw className="h-3 w-3" />
                  </Button>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground">
                <span className="inline-flex items-center gap-1">
                  <Database className="h-3 w-3 text-xblue" />
                  {tx("数据集", "Datasets")} ({fmtNum(rec.dataset_ids.length, 0)}
                  ):
                  {rec.dataset_ids.length === 0 ? (
                    <span>-</span>
                  ) : (
                    rec.dataset_ids.map((dsId) => (
                      <button
                        key={dsId}
                        type="button"
                        onClick={() => onSelectDatasetId?.(dsId)}
                        className="font-mono-tech text-xblue underline cursor-pointer ml-1"
                      >
                        {dsId.slice(0, 8)}
                      </button>
                    ))
                  )}
                </span>

                <span className="inline-flex items-center gap-1">
                  <FileText className="h-3 w-3 text-xblue" />
                  {tx("知识文档", "Document")}:{" "}
                  <span className="font-mono-tech text-foreground">
                    {rec.doc_id ? rec.doc_id.slice(0, 8) : "-"}
                  </span>
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}
