import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  FileText,
  FileUp,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import {
  api,
  type DocumentSummary,
  type IngestResponse,
} from "@/lib/api/rag";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Input,
  PageHeader,
  Pagination,
  Textarea,
} from "@/components/ui/primitives";

const PAGE_SIZE = 20;

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

export function AxIngestPage() {
  const { tx, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canSync = can("DATA_SYNC");

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const dragDepthRef = useRef(0);

  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [dragging, setDragging] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [result, setResult] = useState<IngestResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Knowledge Document Ledger state
  const [docs, setDocs] = useState<DocumentSummary[]>([]);
  const [docTotal, setDocTotal] = useState(0);
  const [docPage, setDocPage] = useState(1);
  const [cursorsByPage, setCursorsByPage] = useState<
    Record<number, string | undefined>
  >({ 1: undefined });
  const [searchInput, setSearchInput] = useState("");
  const [appliedQuery, setAppliedQuery] = useState("");
  const [docsLoading, setDocsLoading] = useState(false);
  const [pendingDeleteDoc, setPendingDeleteDoc] =
    useState<DocumentSummary | null>(null);
  const [deletingDoc, setDeletingDoc] = useState(false);

  const loadDocuments = useCallback(
    async (
      targetPage: number,
      qValue: string,
      cursorMap?: Record<number, string | undefined>
    ) => {
      setDocsLoading(true);
      try {
        const effectiveCursors = cursorMap ?? cursorsByPage;
        const cursor =
          targetPage <= 1 ? undefined : effectiveCursors[targetPage];
        const res = await api.listDocuments({
          q: qValue.trim() || undefined,
          limit: PAGE_SIZE,
          cursor,
        });
        const items = Array.isArray(res?.items) ? res.items : [];
        const total =
          typeof res?.total === "number" ? res.total : items.length;
        setDocs(items);
        setDocTotal(total);
        setDocPage(targetPage);
        if (res?.next_cursor) {
          setCursorsByPage((prev) => ({
            ...(cursorMap ?? prev),
            [targetPage + 1]: res.next_cursor ?? undefined,
          }));
        }
      } catch {
        setDocs([]);
        setDocTotal(0);
      } finally {
        setDocsLoading(false);
      }
    },
    [cursorsByPage]
  );

  useEffect(() => {
    void loadDocuments(1, "", { 1: undefined });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const refreshFirstPage = useCallback(
    async (qValue = appliedQuery) => {
      const resetCursors: Record<number, string | undefined> = { 1: undefined };
      setCursorsByPage(resetCursors);
      await loadDocuments(1, qValue, resetCursors);
    },
    [appliedQuery, loadDocuments]
  );

  const handleTextIngest = async () => {
    if (!canSync || !title.trim() || !content.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await api.ingestText({
        title: title.trim(),
        source_name: `${title.trim()}.md`,
        content: content.trim(),
      });
      setResult(res);
      setTitle("");
      setContent("");
      await refreshFirstPage();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleFileUpload = async () => {
    if (!canSync || !file) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await api.uploadFile(file);
      setResult(res);
      setFile(null);
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
      await refreshFirstPage();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleSearchSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const nextQ = searchInput.trim();
    setAppliedQuery(nextQ);
    void refreshFirstPage(nextQ);
  };

  const handleConfirmDelete = async () => {
    if (!pendingDeleteDoc || !canSync) return;
    setDeletingDoc(true);
    setError(null);
    try {
      await api.deleteDocument(pendingDeleteDoc.id);
      setPendingDeleteDoc(null);
      await refreshFirstPage();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingDoc(false);
    }
  };

  return (
    <div className="space-y-5" data-testid="axingest-page">
      <PageHeader
        title={tx(MODULE_META.axingest.label, MODULE_META.axingest.enLabel)}
        badges={
          <>
            <Badge variant="neutral">
              {tx("文档", "Documents")} {fmtNum(docTotal, 0)}
            </Badge>
            {!canSync && (
              <Badge variant="outline">{tx("只读权限", "Read-Only")}</Badge>
            )}
          </>
        }
      />

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        {/* File Drag-and-Drop Upload Card */}
        <Card className="p-4 space-y-3 flex flex-col justify-between">
          <div className="space-y-3">
            <div className="flex items-center justify-between border-b border-border pb-2.5">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <FileUp className="h-4 w-4 text-xblue" />
                <span>{tx("文件解析入库", "Document File Upload")}</span>
              </div>
              <Badge variant="outline">PDF · DOCX · MD · TXT · PNG</Badge>
            </div>

            <div
              data-testid="ingest-dropzone"
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
                const dropped = e.dataTransfer.files?.[0];
                if (dropped) {
                  setFile(dropped);
                }
              }}
              onClick={() => {
                if (canSync) fileInputRef.current?.click();
              }}
              className={`rounded-md border-2 border-dashed p-6 text-center transition-colors ${
                !canSync
                  ? "border-border bg-muted/20 opacity-60 cursor-not-allowed"
                  : dragging
                    ? "border-xblue bg-xblue-soft/30 cursor-copy"
                    : "border-border bg-muted/15 hover:border-foreground/40 cursor-pointer"
              }`}
            >
              <input
                ref={fileInputRef}
                type="file"
                accept=".pdf,.docx,.md,.txt,.png,.jpg,.jpeg"
                disabled={!canSync}
                data-testid="ingest-file-input"
                onChange={(e) => setFile(e.target.files?.[0] ?? null)}
                className="sr-only"
              />
              <FileUp className="mx-auto mb-2 h-6 w-6 text-xblue" />
              {file ? (
                <div className="inline-flex items-center gap-2 rounded border border-xblue/30 bg-xblue-soft/40 px-3 py-1 text-xs">
                  <span className="font-medium text-foreground truncate max-w-[240px]">
                    {file.name}
                  </span>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    {formatBytes(file.size)}
                  </span>
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      setFile(null);
                      if (fileInputRef.current) {
                        fileInputRef.current.value = "";
                      }
                    }}
                    aria-label={tx("移除文件", "Remove file")}
                    className="text-muted-foreground hover:text-foreground cursor-pointer"
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                </div>
              ) : (
                <div className="space-y-1">
                  <div className="text-xs font-medium text-foreground">
                    {tx("拖拽文件至此，或浏览本地文件", "Drop a document here or browse")}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    .pdf / .docx / .md / .txt / .png / .jpg
                  </div>
                </div>
              )}
            </div>
          </div>

          <div className="flex justify-end pt-1">
            <Button
              type="button"
              onClick={() => void handleFileUpload()}
              disabled={!canSync || !file || submitting}
              data-testid="ingest-upload-submit-btn"
            >
              {submitting
                ? tx("解析中…", "Parsing…")
                : tx("解析入库", "Upload")}
            </Button>
          </div>
        </Card>

        {/* Markdown / Text Direct Ingest Card */}
        <Card className="p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <FileText className="h-4 w-4 text-xblue" />
              <span>{tx("文本直接入库", "Markdown Ingestion")}</span>
            </div>
            <Badge variant="outline">Markdown</Badge>
          </div>

          <div className="space-y-1">
            <label className="text-xs text-muted-foreground block">
              {tx("文档标题", "Document Title")}
            </label>
            <Input
              placeholder={tx(
                "华东仓供应链履约规范",
                "East Warehouse Fulfillment Policy"
              )}
              value={title}
              disabled={!canSync}
              onChange={(e) => setTitle(e.target.value)}
              data-testid="ingest-title-input"
            />
          </div>

          <div className="space-y-1">
            <label className="text-xs text-muted-foreground block">
              {tx("正文内容", "Markdown Content")}
            </label>
            <Textarea
              rows={5}
              placeholder={tx(
                "# 第一章 履约时效\n所有已支付订单须在 24 小时内由华东仓优先拣配出库。",
                "# Section 1 SLA\nAll paid orders must ship within 24 hours."
              )}
              value={content}
              disabled={!canSync}
              onChange={(e) => setContent(e.target.value)}
              data-testid="ingest-content-textarea"
            />
          </div>

          <div className="flex justify-end">
            <Button
              type="button"
              onClick={() => void handleTextIngest()}
              disabled={
                !canSync || !title.trim() || !content.trim() || submitting
              }
              data-testid="ingest-text-submit-btn"
            >
              {submitting
                ? tx("入库中…", "Ingesting…")
                : tx("提交入库", "Ingest")}
            </Button>
          </div>
        </Card>
      </div>

      {error && (
        <Card
          className="p-3.5 border-red-500/30 bg-red-500/5 text-xs"
          data-testid="ingest-error-banner"
        >
          <span className="font-semibold text-red-600 dark:text-red-400">
            {tx("操作失败：", "Operation failed: ")}
          </span>
          <span>{error}</span>
        </Card>
      )}

      {result && (
        <Card className="p-4 space-y-3" data-testid="ingest-result-card">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <CheckCircle2 className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
              <span>
                {tx("入库完成：", "Ingested: ")}
                {result.title}
              </span>
            </div>
            <Badge variant="xblue">{fmtNum(result.embedding_dim, 0)}d</Badge>
          </div>
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-xs">
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">
                {tx("文档标识", "Document ID")}
              </div>
              <div
                className="font-mono-tech mt-1 truncate"
                title={result.doc_id}
              >
                {result.doc_id}
              </div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">
                {tx("段落数", "Paragraphs")}
              </div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {fmtNum(result.para_count, 0)}
              </div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">
                {tx("抽取实体", "Entities")}
              </div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {fmtNum(result.entity_count, 0)}
              </div>
            </div>
            <div className="rounded border border-border p-3">
              <div className="text-muted-foreground">
                {tx("超图边数", "Hyperedges")}
              </div>
              <div className="font-mono-tech text-base font-semibold mt-1">
                {fmtNum(result.edge_count, 0)}
              </div>
            </div>
          </div>
        </Card>
      )}

      {/* Knowledge Document Ledger */}
      <Card className="p-4 space-y-3" data-testid="ingest-document-ledger">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold">
              {tx("知识文档台账", "Knowledge Document Ledger")}
            </h2>
            <Badge variant="neutral">{fmtNum(docTotal, 0)}</Badge>
          </div>

          <form
            onSubmit={handleSearchSubmit}
            className="flex flex-wrap items-center gap-2"
          >
            <Input
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              placeholder={tx("供应链制度", "fulfillment policy")}
              className="h-8 w-48 sm:w-60 text-xs"
              data-testid="ingest-doc-search-input"
            />
            <Button
              type="submit"
              variant="outline"
              size="sm"
              disabled={docsLoading}
              data-testid="ingest-doc-search-btn"
            >
              <Search className="h-3.5 w-3.5" />
              {tx("搜索", "Search")}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={docsLoading}
              onClick={() => void refreshFirstPage()}
              data-testid="ingest-doc-refresh-btn"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${docsLoading ? "animate-spin" : ""}`}
              />
              {tx("刷新", "Refresh")}
            </Button>
          </form>
        </div>

        {docs.length === 0 ? (
          <EmptyState
            title={
              docsLoading
                ? tx("加载中…", "Loading…")
                : tx("暂无知识文档", "No documents ingested")
            }
          />
        ) : (
          <div className="overflow-x-auto rounded border border-border">
            <table
              className="w-full text-left text-xs"
              data-testid="ingest-doc-table"
            >
              <thead className="bg-muted border-b border-border">
                <tr>
                  <th className="px-3 py-2 font-medium text-muted-foreground">
                    {tx("文档标题", "Title")}
                  </th>
                  <th className="px-3 py-2 font-medium text-muted-foreground">
                    {tx("来源文件", "Source")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                    {tx("段落数", "Paragraphs")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                    {tx("实体数", "Entities")}
                  </th>
                  <th className="px-3 py-2 font-medium text-muted-foreground">
                    {tx("更新时间", "Updated")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground">
                    {tx("操作", "Actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {docs.map((doc) => (
                  <tr
                    key={doc.id}
                    data-testid={`ingest-doc-row-${doc.id}`}
                    className="border-b border-border last:border-0 hover:bg-muted/35 transition-colors"
                  >
                    <td className="px-3 py-2.5 max-w-[280px]">
                      <div
                        className="font-medium text-foreground truncate"
                        title={doc.title}
                      >
                        {doc.title}
                      </div>
                      {doc.summary && (
                        <div
                          className="text-[11px] text-muted-foreground truncate mt-0.5"
                          title={doc.summary}
                        >
                          {doc.summary}
                        </div>
                      )}
                    </td>
                    <td
                      className="px-3 py-2.5 font-mono-tech text-muted-foreground max-w-[200px] truncate"
                      title={doc.source_name}
                    >
                      {doc.source_name || "-"}
                    </td>
                    <td className="px-3 py-2.5 text-right font-mono-tech">
                      {fmtNum(doc.para_count, 0)}
                    </td>
                    <td className="px-3 py-2.5 text-right font-mono-tech">
                      {fmtNum(doc.entity_count, 0)}
                    </td>
                    <td className="px-3 py-2.5 font-mono-tech text-muted-foreground whitespace-nowrap">
                      {fmtTime(doc.updated_at || doc.created_at)}
                    </td>
                    <td className="px-3 py-2.5 text-right">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        disabled={!canSync}
                        onClick={() => setPendingDeleteDoc(doc)}
                        data-testid={`ingest-doc-delete-${doc.id}`}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        {tx("删除", "Delete")}
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        <Pagination
          page={docPage}
          pageSize={PAGE_SIZE}
          total={docTotal}
          onPageChange={(nextPage) =>
            void loadDocuments(nextPage, appliedQuery)
          }
        />
      </Card>

      <ConfirmDialog
        open={Boolean(pendingDeleteDoc)}
        onClose={() => setPendingDeleteDoc(null)}
        onConfirm={() => void handleConfirmDelete()}
        loading={deletingDoc}
        title={tx("删除知识文档", "Delete Document")}
        description={tx(
          `即将删除文档「${pendingDeleteDoc?.title ?? ""}」及其关联段落与超图边，删除后不可恢复。`,
          `Delete "${pendingDeleteDoc?.title ?? ""}" and all linked paragraphs and hyperedges permanently.`
        )}
      />
    </div>
  );
}
