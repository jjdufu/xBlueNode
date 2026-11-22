import { useMemo, useState } from "react";
import { Check, Database, FileText, Search, Sparkles } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { Dataset } from "@/lib/api/axhub";
import type { TopologyNode } from "@/lib/api/rag";
import type { AiDraftRequest } from "@/lib/api/xblue";
import {
  Button,
  Input,
  Modal,
  StatText,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface AiDraftWizardProps {
  open: boolean;
  subgraphSlug: string;
  datasets: Dataset[];
  docNodes: TopologyNode[];
  onClose: () => void;
  onGenerate: (req: AiDraftRequest) => Promise<void>;
}

export function AiDraftWizard({
  open,
  subgraphSlug,
  datasets,
  docNodes,
  onClose,
  onGenerate,
}: AiDraftWizardProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [title, setTitle] = useState("AI 供应链本体草案");
  const [selectedDatasetIds, setSelectedDatasetIds] = useState<string[]>([]);
  const [selectedDocIds, setSelectedDocIds] = useState<string[]>([]);
  const [datasetFilter, setDatasetFilter] = useState("");
  const [docFilter, setDocFilter] = useState("");
  const [description, setDescription] = useState("");
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const filteredDatasets = useMemo(() => {
    const q = datasetFilter.trim().toLowerCase();
    if (!q) return datasets;
    return datasets.filter(
      (ds) =>
        ds.name.toLowerCase().includes(q) ||
        (ds.display_name || "").toLowerCase().includes(q)
    );
  }, [datasets, datasetFilter]);

  const filteredDocs = useMemo(() => {
    const q = docFilter.trim().toLowerCase();
    if (!q) return docNodes;
    return docNodes.filter((d) => d.label.toLowerCase().includes(q));
  }, [docNodes, docFilter]);

  const toggleDataset = (id: string) => {
    setSelectedDatasetIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]
    );
  };

  const toggleDoc = (id: string) => {
    setSelectedDocIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]
    );
  };

  const handleSubmit = async () => {
    if (!canEdit) return;
    if (
      selectedDatasetIds.length === 0 &&
      selectedDocIds.length === 0 &&
      description.trim().length === 0
    ) {
      setError(
        tx(
          "需勾选数据集、知识文档或填写领域描述",
          "Select a dataset, document, or enter a domain description"
        )
      );
      return;
    }
    setGenerating(true);
    setError(null);
    try {
      await onGenerate({
        title: title.trim() || tx("AI 建模草案", "AI Schema Draft"),
        dataset_ids: selectedDatasetIds,
        doc_ids: selectedDocIds,
        description: description.trim(),
      });
      onClose();
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : tx("草案生成失败", "Failed to generate draft")
      );
    } finally {
      setGenerating(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="ai-draft-wizard"
      maxWidthClass="max-w-2xl"
      title={
        <>
          <Sparkles className="h-4 w-4 text-xblue shrink-0" />
          <span>
            {tx("AI 草案推导", "AI Draft Generator")} · {subgraphSlug}
          </span>
        </>
      }
      footer={
        <>
          <Button variant="outline" onClick={onClose} disabled={generating}>
            {tx("取消", "Cancel")}
          </Button>
          <Button
            data-testid="ai-draft-submit-btn"
            onClick={() => void handleSubmit()}
            disabled={!canEdit || generating}
          >
            <Sparkles className="h-3.5 w-3.5" />
            {generating
              ? tx("生成中…", "Generating…")
              : tx("生成草案", "Generate Draft")}
          </Button>
        </>
      }
    >
      {error && (
        <div className="rounded border border-red-500/30 bg-red-500/10 p-2.5 text-red-600 dark:text-red-400">
          {error}
        </div>
      )}

      <div className="space-y-3.5 text-xs">
        <div>
          <label className="block font-medium mb-1">
            {tx("草案名称", "Draft Title")}
          </label>
          <Input
            data-testid="ai-draft-title-input"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder={tx("AI 供应链本体草案", "AI Supply Chain Draft")}
            className="h-8 text-xs"
          />
        </div>

        {/* AxHub Datasets Unified Checkbox List */}
        <div className="space-y-1.5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-medium flex items-center gap-1.5">
              <Database className="h-3.5 w-3.5 text-xblue" />
              {tx("关联数据集", "Source Datasets")}
            </span>
            <div className="flex items-center gap-2">
              <StatText
                label={tx("已选", "Selected")}
                value={`${selectedDatasetIds.length}/${datasets.length}`}
              />
              {datasets.length > 0 && (
                <div className="flex items-center gap-1.5 text-[11px]">
                  <button
                    type="button"
                    onClick={() =>
                      setSelectedDatasetIds(
                        filteredDatasets.map((ds) => ds.id)
                      )
                    }
                    className="text-xblue hover:underline cursor-pointer"
                  >
                    {tx("全选", "Select All")}
                  </button>
                  <span className="text-border">·</span>
                  <button
                    type="button"
                    onClick={() => setSelectedDatasetIds([])}
                    className="text-muted-foreground hover:text-foreground cursor-pointer"
                  >
                    {tx("清空", "Clear")}
                  </button>
                </div>
              )}
            </div>
          </div>

          {datasets.length > 4 && (
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-2 h-3.5 w-3.5 text-muted-foreground" />
              <Input
                value={datasetFilter}
                onChange={(e) => setDatasetFilter(e.target.value)}
                placeholder={tx("搜索数据集名称…", "Search datasets…")}
                className="h-7 pl-8 text-xs"
              />
            </div>
          )}

          {datasets.length === 0 ? (
            <div className="rounded border border-dashed border-border p-2.5 text-muted-foreground">
              {tx("暂无可用数据集", "No datasets available")}
            </div>
          ) : (
            <div className="rounded-md border border-border bg-muted/15 p-1.5 max-h-40 overflow-y-auto grid grid-cols-1 md:grid-cols-2 gap-1.5">
              {filteredDatasets.map((ds) => {
                const checked = selectedDatasetIds.includes(ds.id);
                return (
                  <button
                    key={ds.id}
                    type="button"
                    data-testid={`ai-wizard-dataset-${ds.name}`}
                    onClick={() => toggleDataset(ds.id)}
                    className={cn(
                      "flex items-center justify-between gap-2 rounded-md border px-2.5 py-2 text-left transition-colors cursor-pointer",
                      checked
                        ? "border-xblue bg-xblue-soft/60 text-foreground"
                        : "border-border/70 bg-background hover:bg-muted/50"
                    )}
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      <span
                        className={cn(
                          "inline-flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors",
                          checked
                            ? "border-xblue bg-xblue text-white"
                            : "border-input bg-background"
                        )}
                      >
                        {checked && <Check className="h-3 w-3" />}
                      </span>
                      <div className="min-w-0">
                        <div className="font-medium truncate">
                          {ds.display_name || ds.name}
                        </div>
                        <div className="font-mono-tech text-[10px] text-muted-foreground truncate">
                          {ds.name} · {fmtNum(ds.row_count, 0)}{" "}
                          {tx("行", "rows")}
                        </div>
                      </div>
                    </div>
                    <StatusBadge status={ds.status} />
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* Knowledge Documents Unified Checkbox List */}
        <div className="space-y-1.5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-medium flex items-center gap-1.5">
              <FileText className="h-3.5 w-3.5 text-xblue" />
              {tx("知识文档抽取", "Knowledge Documents")}
            </span>
            <div className="flex items-center gap-2">
              <StatText
                label={tx("已选", "Selected")}
                value={`${selectedDocIds.length}/${docNodes.length}`}
              />
              {docNodes.length > 0 && (
                <div className="flex items-center gap-1.5 text-[11px]">
                  <button
                    type="button"
                    onClick={() =>
                      setSelectedDocIds(filteredDocs.map((d) => d.id))
                    }
                    className="text-xblue hover:underline cursor-pointer"
                  >
                    {tx("全选", "Select All")}
                  </button>
                  <span className="text-border">·</span>
                  <button
                    type="button"
                    onClick={() => setSelectedDocIds([])}
                    className="text-muted-foreground hover:text-foreground cursor-pointer"
                  >
                    {tx("清空", "Clear")}
                  </button>
                </div>
              )}
            </div>
          </div>

          {docNodes.length > 4 && (
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-2 h-3.5 w-3.5 text-muted-foreground" />
              <Input
                value={docFilter}
                onChange={(e) => setDocFilter(e.target.value)}
                placeholder={tx("搜索知识文档标题…", "Search documents…")}
                className="h-7 pl-8 text-xs"
              />
            </div>
          )}

          {docNodes.length === 0 ? (
            <div className="rounded border border-dashed border-border p-2.5 text-muted-foreground">
              {tx("暂无已入库文档", "No ingested documents")}
            </div>
          ) : (
            <div className="rounded-md border border-border bg-muted/15 p-1.5 max-h-36 overflow-y-auto grid grid-cols-1 md:grid-cols-2 gap-1.5">
              {filteredDocs.map((doc) => {
                const checked = selectedDocIds.includes(doc.id);
                return (
                  <button
                    key={doc.id}
                    type="button"
                    onClick={() => toggleDoc(doc.id)}
                    className={cn(
                      "flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-left transition-colors cursor-pointer",
                      checked
                        ? "border-xblue bg-xblue-soft/60 text-foreground"
                        : "border-border/70 bg-background hover:bg-muted/50"
                    )}
                  >
                    <span
                      className={cn(
                        "inline-flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors",
                        checked
                          ? "border-xblue bg-xblue text-white"
                          : "border-input bg-background"
                      )}
                    >
                      {checked && <Check className="h-3 w-3" />}
                    </span>
                    <span className="truncate font-medium">{doc.label}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* Natural Language Description */}
        <div>
          <label className="block font-medium mb-1">
            {tx("业务领域描述", "Domain Description")}
          </label>
          <Textarea
            data-testid="ai-draft-description-input"
            rows={2}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder={tx(
              "补充上游供应商 Supplier 与仓库补货 SUPPLIES 关系",
              "Add upstream Supplier nodes and SUPPLIES edges to Warehouse"
            )}
            className="text-xs"
          />
        </div>
      </div>
    </Modal>
  );
}
