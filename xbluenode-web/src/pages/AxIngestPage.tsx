import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  Check,
  CheckCircle2,
  Copy,
  FilePenLine,
  FileText,
  FileUp,
  Loader2,
  Plus,
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
  type FusedHit,
  type IngestResponse,
  type TopologyEdge,
  type TopologyNode,
} from "@/lib/api/rag";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Drawer,
  EmptyState,
  Input,
  Modal,
  PageHeader,
  Pagination,
  StatText,
  Tabs,
  Textarea,
} from "@/components/ui/primitives";

const PAGE_SIZE = 20;
const SUPPORTED_FORMATS_LABEL = "PDF · DOCX · MD · TXT · PNG · JPG";

type IngestModalTab = "upload" | "markdown";
type InspectorTab = "chunks" | "entities";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function translateHyperRel(rel: string, isZh: boolean): string {
  const upper = rel.toUpperCase();
  if (!isZh) return rel;
  switch (upper) {
    case "MENTIONS":
      return "提及实体";
    case "HAS_PARA":
    case "CONTAINS":
      return "包含段落";
    case "CO_OCCURS":
      return "同段共现";
    case "RELATED_TO":
      return "语义关联";
    default:
      return rel;
  }
}

export function AxIngestPage() {
  const { isZh, tx, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canSync = can("DATA_SYNC");

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const dragDepthRef = useRef(0);

  // Create / Re-ingest Modal state
  const [ingestModalOpen, setIngestModalOpen] = useState(false);
  const [ingestTab, setIngestTab] = useState<IngestModalTab>("upload");
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
  const [copiedId, setCopiedId] = useState<string | null>(null);

  // Document Inspector Drawer state
  const [inspectDoc, setInspectDoc] = useState<DocumentSummary | null>(null);
  const [inspectTab, setInspectTab] = useState<InspectorTab>("chunks");
  const [inspectLoading, setInspectLoading] = useState(false);
  const [inspectChunks, setInspectChunks] = useState<FusedHit[]>([]);
  const [inspectEntities, setInspectEntities] = useState<TopologyNode[]>([]);
  const [inspectEdges, setInspectEdges] = useState<TopologyEdge[]>([]);

  // Delete confirmation
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

  const openInspectDrawer = useCallback(
    async (doc: DocumentSummary, initialTab: InspectorTab = "chunks") => {
      setInspectDoc(doc);
      setInspectTab(initialTab);
      setInspectLoading(true);
      setInspectChunks([]);
      setInspectEntities([]);
      setInspectEdges([]);
      try {
        const [queryRes, topoRes] = await Promise.all([
          api
            .chatQuery({ query: doc.title, mode: "DOC", top_k: 16 })
            .catch(() => null),
          api.topology().catch(() => null),
        ]);

        const allHits = Array.isArray(queryRes?.hits) ? queryRes.hits : [];
        const matchedHits = allHits.filter(
          (h) => h.doc_id === doc.id || h.doc_title === doc.title
        );
        setInspectChunks(matchedHits.length > 0 ? matchedHits : allHits.slice(0, 6));

        if (topoRes && Array.isArray(topoRes.nodes) && Array.isArray(topoRes.edges)) {
          const docNode = topoRes.nodes.find(
            (n) =>
              n.id === doc.id ||
              (n.node_type === "DOC" && n.label.includes(doc.title))
          );
          if (docNode) {
            const linkedEdges = topoRes.edges.filter(
              (e) => e.source === docNode.id || e.target === docNode.id
            );
            const neighborIds = new Set<string>();
            for (const edge of linkedEdges) {
              if (edge.source !== docNode.id) neighborIds.add(edge.source);
              if (edge.target !== docNode.id) neighborIds.add(edge.target);
            }
            const entities = topoRes.nodes.filter(
              (n) => neighborIds.has(n.id) && n.node_type !== "DOC"
            );
            setInspectEdges(linkedEdges);
            setInspectEntities(entities);
          }
        }
      } finally {
        setInspectLoading(false);
      }
    },
    []
  );

  const handleOpenEditReingest = (doc: DocumentSummary) => {
    const assembledBody =
      inspectChunks.length > 0
        ? inspectChunks.map((c) => c.content.trim()).join("\n\n")
        : doc.summary || "";
    setTitle(doc.title);
    setContent(assembledBody);
    setIngestTab("markdown");
    setInspectDoc(null);
    setIngestModalOpen(true);
  };

  const handleCopySource = (id: string, text: string) => {
    if (!text) return;
    void navigator.clipboard?.writeText(text);
    setCopiedId(id);
    setTimeout(() => {
      setCopiedId((prev) => (prev === id ? null : prev));
    }, 1500);
  };

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
      setIngestModalOpen(false);
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
      setIngestModalOpen(false);
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
      const deletedId = pendingDeleteDoc.id;
      await api.deleteDocument(deletedId);
      setPendingDeleteDoc(null);
      if (inspectDoc?.id === deletedId) {
        setInspectDoc(null);
      }
      await refreshFirstPage();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingDoc(false);
    }
  };

  return (
    <div className="space-y-3.5" data-testid="axingest-page">
      <PageHeader
        title={tx(MODULE_META.axingest.label, MODULE_META.axingest.enLabel)}
        badges={
          <div className="flex items-center gap-2.5 pl-1">
            <StatText
              label={tx("文档", "Documents")}
              value={fmtNum(docTotal, 0)}
            />
            {!canSync && (
              <Badge variant="outline">{tx("只读权限", "Read-Only")}</Badge>
            )}
          </div>
        }
        actions={
          canSync ? (
            <Button
              type="button"
              onClick={() => {
                setError(null);
                setIngestModalOpen(true);
              }}
              data-testid="open-ingest-modal-btn"
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新增文档", "Ingest Document")}
            </Button>
          ) : undefined
        }
      />

      {error && (
        <Card
          className="p-3 border-red-500/30 bg-red-500/5 text-xs flex items-center justify-between"
          data-testid="ingest-error-banner"
        >
          <div>
            <span className="font-semibold text-red-600 dark:text-red-400">
              {tx("操作失败：", "Operation failed: ")}
            </span>
            <span>{error}</span>
          </div>
          <button
            type="button"
            onClick={() => setError(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {tx("关闭", "Dismiss")}
          </button>
        </Card>
      )}

      {result && (
        <Card
          className="px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-3 border-emerald-500/30 bg-emerald-500/5 text-xs"
          data-testid="ingest-result-card"
        >
          <div className="flex flex-wrap items-center gap-2">
            <CheckCircle2 className="h-4 w-4 text-emerald-600 dark:text-emerald-400 shrink-0" />
            <span className="font-semibold">
              {tx("入库完成：", "Ingested: ")}
              {result.title}
            </span>
            <span className="font-mono-tech text-muted-foreground">
              {tx("段落", "Chunks")} {fmtNum(result.para_count, 0)} ·{" "}
              {tx("实体", "Entities")} {fmtNum(result.entity_count, 0)} ·{" "}
              {tx("超边", "Hyperedges")} {fmtNum(result.edge_count, 0)} ·{" "}
              {fmtNum(result.embedding_dim, 0)}d
            </span>
          </div>
          <button
            type="button"
            onClick={() => setResult(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </Card>
      )}

      {/* Primary Above-the-Fold Knowledge Document Ledger */}
      <Card className="p-3.5 space-y-3" data-testid="ingest-document-ledger">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-sm font-semibold mr-1">
              {tx("知识文档台账", "Knowledge Document Ledger")}
            </h2>

            <form
              onSubmit={handleSearchSubmit}
              className="flex flex-wrap items-center gap-2"
            >
              <div className="relative w-56 sm:w-68">
                <Search className="pointer-events-none absolute left-2.5 top-2.25 h-3.5 w-3.5 text-muted-foreground" />
                <Input
                  value={searchInput}
                  onChange={(e) => setSearchInput(e.target.value)}
                  placeholder={tx(
                    "搜索文档标题或摘要",
                    "Search document title or summary"
                  )}
                  className="pl-8"
                  data-testid="ingest-doc-search-input"
                />
              </div>
              <Button
                type="submit"
                variant="outline"
                disabled={docsLoading}
                data-testid="ingest-doc-search-btn"
              >
                {tx("搜索", "Search")}
              </Button>
              {appliedQuery && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setSearchInput("");
                    setAppliedQuery("");
                    void refreshFirstPage("");
                  }}
                >
                  <X className="h-3 w-3" />
                  {tx("重置", "Reset")}
                </Button>
              )}
            </form>
          </div>

          <Button
            type="button"
            variant="ghost"
            disabled={docsLoading}
            onClick={() => void refreshFirstPage()}
            data-testid="ingest-doc-refresh-btn"
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${docsLoading ? "animate-spin" : ""}`}
            />
            {tx("刷新", "Refresh")}
          </Button>
        </div>

        {docs.length === 0 ? (
          <EmptyState
            title={
              docsLoading
                ? tx("加载中…", "Loading…")
                : tx("暂无知识文档", "No documents ingested")
            }
            action={
              !docsLoading && canSync ? (
                <Button size="sm" onClick={() => setIngestModalOpen(true)}>
                  <Plus className="h-3.5 w-3.5" />
                  {tx("新增文档", "Ingest Document")}
                </Button>
              ) : undefined
            }
          />
        ) : (
          <div className="overflow-x-auto rounded border border-border">
            <table
              className="w-full text-left text-xs"
              data-testid="ingest-doc-table"
            >
              <thead className="bg-muted/60 border-b border-border">
                <tr>
                  <th className="px-3 py-2 font-medium text-muted-foreground">
                    {tx("文档标题", "Title")}
                  </th>
                  <th className="px-3 py-2 font-medium text-muted-foreground">
                    {tx("来源文件", "Source")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground whitespace-nowrap">
                    {tx("段落数", "Chunks")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground whitespace-nowrap">
                    {tx("实体数", "Entities")}
                  </th>
                  <th className="px-3 py-2 font-medium text-muted-foreground whitespace-nowrap">
                    {tx("更新时间", "Updated")}
                  </th>
                  <th className="px-3 py-2 text-right font-medium text-muted-foreground whitespace-nowrap">
                    {tx("操作", "Actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {docs.map((doc) => {
                  const activeInspect = inspectDoc?.id === doc.id;
                  return (
                    <tr
                      key={doc.id}
                      onClick={() => void openInspectDrawer(doc, "chunks")}
                      data-testid={`ingest-doc-row-${doc.id}`}
                      className={`border-b border-border last:border-0 cursor-pointer transition-colors ${
                        activeInspect
                          ? "bg-xblue-soft/55"
                          : "hover:bg-muted/35"
                      }`}
                    >
                      <td className="px-3 py-2 max-w-[340px]">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            void openInspectDrawer(doc, "chunks");
                          }}
                          className="font-medium text-foreground hover:text-xblue hover:underline truncate block max-w-full text-left cursor-pointer"
                          title={doc.title}
                        >
                          {doc.title}
                        </button>
                        {doc.summary && (
                          <div
                            className="text-[11px] text-muted-foreground truncate mt-0.5"
                            title={doc.summary}
                          >
                            {doc.summary}
                          </div>
                        )}
                      </td>

                      <td className="px-3 py-2 max-w-[220px]">
                        {doc.source_name ? (
                          <div className="inline-flex items-center gap-1 max-w-full group">
                            <span
                              className="font-mono-tech text-muted-foreground truncate"
                              title={doc.source_name}
                            >
                              {doc.source_name}
                            </span>
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                handleCopySource(doc.id, doc.source_name);
                              }}
                              title={tx("复制来源名称", "Copy source name")}
                              aria-label={tx("复制来源名称", "Copy source name")}
                              className="opacity-0 group-hover:opacity-100 rounded p-0.5 text-muted-foreground hover:text-foreground hover:bg-muted transition-opacity shrink-0 cursor-pointer"
                            >
                              {copiedId === doc.id ? (
                                <Check className="h-3 w-3 text-emerald-600" />
                              ) : (
                                <Copy className="h-3 w-3" />
                              )}
                            </button>
                          </div>
                        ) : (
                          <span className="text-muted-foreground">-</span>
                        )}
                      </td>

                      <td className="px-3 py-2 text-right font-mono-tech tabular-nums">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            void openInspectDrawer(doc, "chunks");
                          }}
                          className="rounded px-1.5 py-0.5 text-xblue hover:bg-xblue-soft/70 cursor-pointer"
                          title={tx("核验切片段落", "Inspect chunks")}
                        >
                          {fmtNum(doc.para_count, 0)}
                        </button>
                      </td>

                      <td className="px-3 py-2 text-right font-mono-tech tabular-nums">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            void openInspectDrawer(doc, "entities");
                          }}
                          className="rounded px-1.5 py-0.5 text-xblue hover:bg-xblue-soft/70 cursor-pointer"
                          title={tx("查看抽取实体", "Inspect entities")}
                        >
                          {fmtNum(doc.entity_count, 0)}
                        </button>
                      </td>

                      <td className="px-3 py-2 font-mono-tech tabular-nums text-muted-foreground whitespace-nowrap">
                        {fmtTime(doc.updated_at || doc.created_at)}
                      </td>

                      <td className="px-3 py-2 text-right whitespace-nowrap">
                        <div className="inline-flex items-center justify-end gap-1">
                          <button
                            type="button"
                            onClick={(e) => {
                              e.stopPropagation();
                              void openInspectDrawer(doc, "chunks");
                            }}
                            className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs font-medium text-xblue hover:bg-xblue-soft/70 transition-colors cursor-pointer"
                            data-testid={`ingest-doc-inspect-${doc.id}`}
                          >
                            {tx("切片详情", "Inspect")}
                          </button>

                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            disabled={!canSync}
                            onClick={(e) => {
                              e.stopPropagation();
                              setPendingDeleteDoc(doc);
                            }}
                            aria-label={tx("删除", "Delete")}
                            title={tx("删除", "Delete")}
                            className="text-muted-foreground hover:text-red-600 dark:hover:text-red-400 hover:bg-red-500/10"
                            data-testid={`ingest-doc-delete-${doc.id}`}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
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

      {/* Dual-Tab Document Ingestion Modal (File Upload vs Markdown Text) */}
      <Modal
        open={ingestModalOpen}
        onClose={() => setIngestModalOpen(false)}
        maxWidthClass="max-w-xl"
        testId="ingest-create-modal"
        title={
          <>
            <FileUp className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("新增知识文档", "Ingest Document")}</span>
          </>
        }
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => setIngestModalOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            {ingestTab === "upload" ? (
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
            ) : (
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
            )}
          </>
        }
      >
        <Tabs
          value={ingestTab}
          onChange={setIngestTab}
          items={[
            {
              key: "upload",
              label: tx("文件解析入库", "File Upload"),
              icon: <FileUp className="h-3.5 w-3.5" />,
            },
            {
              key: "markdown",
              label: tx("文本直接入库", "Markdown Text"),
              icon: <FileText className="h-3.5 w-3.5" />,
            },
          ]}
        />

        {ingestTab === "upload" ? (
          <div className="space-y-3 pt-1">
            <div className="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>{tx("支持格式", "Supported formats")}</span>
              <span className="font-mono-tech">{SUPPORTED_FORMATS_LABEL}</span>
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
              className={`rounded-md border-2 border-dashed p-7 text-center transition-colors ${
                !canSync
                  ? "border-border bg-muted/20 opacity-60 cursor-not-allowed"
                  : dragging
                    ? "border-xblue bg-xblue-soft/30 cursor-copy"
                    : "border-border bg-muted/15 hover:border-xblue/60 cursor-pointer"
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
                  <span className="font-medium text-foreground truncate max-w-[260px]">
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
                    {tx(
                      "拖拽文件至此，或浏览本地文件",
                      "Drop a document here or browse"
                    )}
                  </div>
                  <div className="text-[11px] font-mono-tech text-muted-foreground">
                    {SUPPORTED_FORMATS_LABEL}
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="space-y-3 pt-1">
            <div className="space-y-1">
              <label className="text-xs font-medium block">
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
              <label className="text-xs font-medium block">
                {tx("正文内容", "Markdown Content")}
              </label>
              <Textarea
                rows={7}
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
          </div>
        )}
      </Modal>

      {/* Document Chunk & Entity Verification Drawer */}
      <Drawer
        open={Boolean(inspectDoc)}
        onClose={() => setInspectDoc(null)}
        widthClass="max-w-2xl"
        testId="ingest-doc-inspector"
        title={
          inspectDoc ? (
            <span className="truncate">{inspectDoc.title}</span>
          ) : (
            tx("文档详情", "Document Details")
          )
        }
        footer={
          inspectDoc ? (
            <div className="w-full flex items-center justify-between gap-2">
              {canSync ? (
                <Button
                  type="button"
                  variant="danger"
                  size="sm"
                  onClick={() => setPendingDeleteDoc(inspectDoc)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  {tx("删除文档", "Delete")}
                </Button>
              ) : (
                <span />
              )}
              <div className="flex items-center gap-2">
                {canSync && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => handleOpenEditReingest(inspectDoc)}
                  >
                    <FilePenLine className="h-3.5 w-3.5 text-xblue" />
                    {tx("编辑重入库", "Edit & Re-ingest")}
                  </Button>
                )}
                <Button
                  type="button"
                  size="sm"
                  onClick={() => setInspectDoc(null)}
                >
                  {tx("关闭", "Close")}
                </Button>
              </div>
            </div>
          ) : undefined
        }
      >
        {inspectDoc && (
          <div className="space-y-3">
            {/* Metadata Summary */}
            <div className="rounded-md bg-muted/35 p-3 space-y-1.5 text-xs">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="font-mono-tech text-muted-foreground truncate">
                  {tx("来源", "Source")}: {inspectDoc.source_name || "-"}
                </span>
                <span className="font-mono-tech tabular-nums text-muted-foreground">
                  {fmtTime(inspectDoc.updated_at || inspectDoc.created_at)}
                </span>
              </div>
              {inspectDoc.summary && (
                <p className="text-foreground leading-relaxed">
                  {inspectDoc.summary}
                </p>
              )}
              <div className="flex items-center gap-4 pt-1 text-[11px] text-muted-foreground">
                <StatText
                  label={tx("切片段落", "Chunks")}
                  value={fmtNum(inspectDoc.para_count, 0)}
                />
                <StatText
                  label={tx("抽取实体", "Entities")}
                  value={fmtNum(inspectDoc.entity_count, 0)}
                />
              </div>
            </div>

            <Tabs
              value={inspectTab}
              onChange={setInspectTab}
              items={[
                {
                  key: "chunks",
                  label: tx("切片段落核验", "Parsed Chunks"),
                  count: inspectDoc.para_count,
                },
                {
                  key: "entities",
                  label: tx("抽取实体与超边", "Extracted Entities"),
                  count: inspectDoc.entity_count,
                },
              ]}
            />

            {inspectLoading ? (
              <div className="py-10 flex items-center justify-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin text-xblue" />
                <span>{tx("加载切片与实体…", "Loading chunks & entities…")}</span>
              </div>
            ) : inspectTab === "chunks" ? (
              inspectChunks.length === 0 ? (
                <EmptyState
                  title={tx("暂无可展示段落切片", "No paragraph chunks found")}
                />
              ) : (
                <div className="space-y-2">
                  {inspectChunks.map((chunk, idx) => (
                    <div
                      key={chunk.para_id}
                      className="rounded-md border border-border bg-background p-3 space-y-1.5"
                    >
                      <div className="flex items-center justify-between gap-2 text-[11px]">
                        <span className="font-mono-tech font-semibold text-xblue">
                          #{idx + 1} · {chunk.para_id.slice(0, 8)}
                        </span>
                        <span className="font-mono-tech text-muted-foreground">
                          {chunk.content.length} {tx("字", "chars")}
                        </span>
                      </div>
                      <p className="text-xs text-foreground leading-relaxed whitespace-pre-wrap">
                        {chunk.content}
                      </p>
                    </div>
                  ))}
                </div>
              )
            ) : inspectEntities.length === 0 && inspectEdges.length === 0 ? (
              <EmptyState
                title={tx(
                  "当前超图采样中未命中该文档的关联实体",
                  "No linked hypergraph entities in active sample"
                )}
              />
            ) : (
              <div className="space-y-3">
                {inspectEntities.length > 0 && (
                  <div className="flex flex-wrap gap-1.5">
                    {inspectEntities.map((ent) => (
                      <Badge key={ent.id} variant="xblue">
                        {ent.label}
                      </Badge>
                    ))}
                  </div>
                )}
                {inspectEdges.length > 0 && (
                  <div className="space-y-1.5">
                    {inspectEdges.slice(0, 20).map((edge) => (
                      <div
                        key={edge.id}
                        className="rounded border border-border bg-background px-3 py-2 text-xs space-y-0.5"
                      >
                        <div className="flex items-center justify-between">
                          <Badge variant="outline">
                            {translateHyperRel(edge.rel_type, isZh)}
                          </Badge>
                        </div>
                        {edge.fact && (
                          <p className="text-muted-foreground leading-relaxed pt-0.5">
                            {edge.fact}
                          </p>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </Drawer>

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
