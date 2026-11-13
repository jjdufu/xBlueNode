import { useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  Database,
  FileText,
  FileUp,
  Loader2,
  RefreshCw,
  XCircle,
} from "lucide-react";
import { axhubApi, type UploadRecord } from "@/lib/api/axhub";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

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
  const fileInputRef = useRef<HTMLInputElement | null>(null);
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

  // Poll PENDING / PARSED uploads until COMMITTED or FAILED
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

  return (
    <Card className="p-4 space-y-4" data-testid="upload-dropzone-panel">
      <div className="border-b border-border pb-3">
        <h2 className="text-sm font-semibold">双轨文件解析与一键入湖</h2>
        <p className="text-xs text-muted-foreground">
          支持 CSV / TSV / XLSX / XLS / DOCX / PPTX / PDF / JSON / JSONL / XML / LOG 等多格式文件，自动完成表格轨写 DuckLake 数据集与文本轨写超图知识库
        </p>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs flex items-center justify-between"
          data-testid="upload-error-banner"
        >
          <span>{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            关闭
          </button>
        </div>
      )}

      {/* Upload Options */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-3 text-xs">
        <div className="space-y-1">
          <label className="text-muted-foreground block">数据集前缀（可选）</label>
          <Input
            value={datasetPrefix}
            onChange={(e) => setDatasetPrefix(e.target.value)}
            placeholder="默认按文件名规范化"
            data-testid="upload-prefix-input"
          />
        </div>

        <div className="space-y-1">
          <label className="text-muted-foreground block">日志格式提示</label>
          <select
            value={logFormat}
            onChange={(e) => setLogFormat(e.target.value)}
            className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-xs"
          >
            <option value="">自动探测</option>
            <option value="nginx">Nginx Combined</option>
            <option value="log4j">Java / Log4j 多行堆栈</option>
            <option value="syslog">RFC 3164 Syslog</option>
            <option value="json">容器 JSON 日志</option>
          </select>
        </div>

        <div className="flex flex-col justify-center gap-1.5 pt-2">
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={toDatasets}
              onChange={(e) => setToDatasets(e.target.checked)}
              data-testid="upload-to-datasets"
            />
            表格轨写入 DuckLake 数据集
          </label>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={toKnowledge}
              onChange={(e) => setToKnowledge(e.target.checked)}
              data-testid="upload-to-knowledge"
            />
            文本轨写入超图知识库
          </label>
        </div>

        <div className="flex flex-col justify-center gap-1.5 pt-2">
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={jsonExplode}
              onChange={(e) => setJsonExplode(e.target.checked)}
            />
            嵌套 JSON 数组拆分子表
          </label>
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={ocrEnabled}
              onChange={(e) => setOcrEnabled(e.target.checked)}
            />
            启用扫描件 OCR 识别
          </label>
        </div>
      </div>

      {/* Dropzone */}
      <div
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          if (e.dataTransfer.files.length > 0) {
            void handleFiles(e.dataTransfer.files);
          }
        }}
        onClick={() => fileInputRef.current?.click()}
        className={`rounded-md border-2 border-dashed p-8 text-center transition-colors cursor-pointer ${
          dragging
            ? "border-xblue bg-xblue-soft"
            : "border-border hover:border-foreground/40 bg-muted/20"
        }`}
        data-testid="upload-dropzone"
      >
        <input
          ref={fileInputRef}
          type="file"
          multiple
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
            ? "正在流式上传并入队解析任务..."
            : "拖拽文件到此处，或点击选择本地文件上传"}
        </div>
        <div className="text-[11px] text-muted-foreground mt-1">
          同租户相同 SHA-256 已提交文件将自动幂等复用已有资产
        </div>
      </div>

      {/* Upload Records */}
      {uploads.length > 0 && (
        <div className="space-y-2" data-testid="upload-records-list">
          <div className="text-xs font-medium">上传与入湖任务追踪 ({uploads.length})</div>
          {uploads.map((rec) => (
            <div
              key={rec.id}
              className="rounded border border-border p-3 text-xs space-y-2 bg-background"
              data-testid={`upload-record-${rec.filename}`}
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{rec.filename}</span>
                  <span className="font-mono-tech text-muted-foreground">
                    {formatBytes(rec.size_bytes)} · sha256:{rec.sha256.slice(0, 10)}
                  </span>
                  {dedupFlags[rec.id] && (
                    <Badge variant="outline">SHA-256 去重命中</Badge>
                  )}
                </div>

                <div className="flex items-center gap-2">
                  {rec.status === "COMMITTED" ? (
                    <Badge variant="xblue" className="gap-1">
                      <CheckCircle2 className="h-3 w-3" />
                      COMMITTED
                    </Badge>
                  ) : rec.status === "FAILED" ? (
                    <Badge variant="outline" className="gap-1">
                      <XCircle className="h-3 w-3" />
                      FAILED {rec.error_code ? `(${rec.error_code})` : ""}
                    </Badge>
                  ) : (
                    <Badge variant="neutral" className="gap-1">
                      <Loader2 className="h-3 w-3 animate-spin" />
                      {rec.status}
                    </Badge>
                  )}

                  <Button
                    type="button"
                    variant="ghost"
                    className="px-2 py-1 text-[11px]"
                    onClick={() => void refreshSingle(rec.id)}
                  >
                    <RefreshCw className="h-3 w-3" />
                  </Button>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground">
                <span className="inline-flex items-center gap-1">
                  <Database className="h-3 w-3 text-xblue" />
                  生成数据集 ({rec.dataset_ids.length}):
                  {rec.dataset_ids.length === 0 ? (
                    <span>暂无</span>
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
                  关联知识库文档:{" "}
                  <span className="font-mono-tech text-foreground">
                    {rec.doc_id ?? "未生成"}
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
