import { useState } from "react";
import { Upload, FileText, CheckCircle2 } from "lucide-react";
import { api, type IngestResponse } from "@/lib/api";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

export function AxIngestPage() {
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [result, setResult] = useState<IngestResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleTextIngest = async () => {
    if (!title.trim() || !content.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await api.ingestText({
        title: title.trim(),
        source_name: `${title.trim()}.md`,
        content: content.trim(),
      });
      setResult(res);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleFileUpload = async () => {
    if (!file) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await api.uploadFile(file);
      setResult(res);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="space-y-6">
      <div className="border-b border-border pb-4">
        <h1 className="text-lg font-semibold tracking-tight">AxIngest · 知识解析入库</h1>
        <p className="text-xs text-muted-foreground mt-0.5">
          上传 PDF/Word/Markdown 文件（经 AxParser 解析），或直接输入 Markdown 文本构建三级超图
        </p>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-5">
        <Card className="p-5 space-y-4">
          <div className="flex items-center gap-2 text-sm font-medium">
            <Upload className="h-4 w-4 text-xblue" />
            文件上传解析入库
          </div>
          <p className="text-xs text-muted-foreground">
            支持 `.pdf`、`.docx`、`.md`、`.txt`、`.png`、`.jpg`，自动提取版面、表格与实体关系。
          </p>
          <input
            type="file"
            accept=".pdf,.docx,.md,.txt,.png,.jpg,.jpeg"
            onChange={(e) => setFile(e.target.files?.[0] ?? null)}
            className="block w-full text-xs file:mr-3 file:rounded file:border file:border-border file:bg-muted file:px-3 file:py-1.5 file:text-xs file:font-medium cursor-pointer"
          />
          <Button
            onClick={() => void handleFileUpload()}
            disabled={!file || submitting}
          >
            {submitting ? "正在解析与抽取三元组..." : "上传并写入超图"}
          </Button>
        </Card>

        <Card className="p-5 space-y-3">
          <div className="flex items-center gap-2 text-sm font-medium">
            <FileText className="h-4 w-4 text-xblue" />
            Markdown / 文本直接入库
          </div>
          <Input
            placeholder="文档标题（例如：供应链调度规则手册）"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
          <Textarea
            rows={6}
            placeholder="在此粘贴 Markdown 或业务文本..."
            value={content}
            onChange={(e) => setContent(e.target.value)}
          />
          <Button
            onClick={() => void handleTextIngest()}
            disabled={!title.trim() || !content.trim() || submitting}
          >
            {submitting ? "正在向量化与入库..." : "提交写入超图"}
          </Button>
        </Card>
      </div>

      {error && (
        <Card className="p-4 border-foreground/40 bg-muted/40 text-xs">
          <span className="font-mono-tech font-semibold">入库失败：</span>
          {error}
        </Card>
      )}

      {result && (
        <Card className="p-5 space-y-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm font-medium">
              <CheckCircle2 className="h-4 w-4 text-xblue" />
              超图写入完成：《{result.title}》
            </div>
            <Badge variant="xblue">{result.embedding_dim}d</Badge>
          </div>
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs">
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">文档 ID</div>
              <div className="font-mono-tech mt-1 truncate">{result.doc_id}</div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">段落切块</div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {result.para_count}
              </div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">抽取实体</div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {result.entity_count}
              </div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">超图边数</div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {result.edge_count}
              </div>
            </div>
          </div>
        </Card>
      )}
    </div>
  );
}
