import { useState } from "react";
import { Database, FileText, Sparkles, X } from "lucide-react";
import type { Dataset } from "@/lib/api/axhub";
import type { TopologyNode } from "@/lib/api/rag";
import type { AiDraftRequest } from "@/lib/api/xblue";
import { Badge, Button, Input, Textarea } from "@/components/ui/primitives";
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
  const [title, setTitle] = useState("AI 供应链本体草案");
  const [selectedDatasetIds, setSelectedDatasetIds] = useState<string[]>([]);
  const [selectedDocIds, setSelectedDocIds] = useState<string[]>([]);
  const [description, setDescription] = useState("");
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!open) return null;

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
    if (
      selectedDatasetIds.length === 0 &&
      selectedDocIds.length === 0 &&
      description.trim().length === 0
    ) {
      setError("请至少勾选一个 AxHub 数据集、知识文档，或输入自然语言业务描述");
      return;
    }
    setGenerating(true);
    setError(null);
    try {
      await onGenerate({
        title: title.trim() || "AI 智能建模草案",
        dataset_ids: selectedDatasetIds,
        doc_ids: selectedDocIds,
        description: description.trim(),
      });
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : "AI 草案生成失败");
    } finally {
      setGenerating(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      data-testid="ai-draft-wizard"
    >
      <div className="w-full max-w-2xl max-h-[88vh] flex flex-col rounded-md border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <Sparkles className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">
                AI 辅助模型草案向导 · 目标子图 ({subgraphSlug})
              </h2>
              <p className="text-[11px] text-muted-foreground">
                基于 DuckDB 列画像/外键推断、知识超图实体及业务描述自动生成 SchemaDoc 草案
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 hover:bg-muted text-muted-foreground cursor-pointer"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
          {error && (
            <div className="rounded border border-red-500/30 bg-red-500/10 p-2.5 text-red-600 dark:text-red-400">
              {error}
            </div>
          )}

          <div>
            <label className="block font-medium mb-1">草案名称 (title)</label>
            <Input
              data-testid="ai-draft-title-input"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="例如 华东供应链履约智能建模草案"
              className="h-8 text-xs"
            />
          </div>

          {/* AxHub Datasets Selection */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <span className="font-medium flex items-center gap-1.5">
                <Database className="h-3.5 w-3.5 text-xblue" />
                1. 勾选 AxHub 数据集进行确定性外键与列画像预分析 (
                {selectedDatasetIds.length}/{datasets.length})
              </span>
            </div>
            {datasets.length === 0 ? (
              <div className="rounded border border-dashed border-border p-2.5 text-muted-foreground">
                暂无可用数据集，可直接通过下方业务描述生成草案。
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5 max-h-36 overflow-y-auto">
                {datasets.map((ds) => {
                  const checked = selectedDatasetIds.includes(ds.id);
                  return (
                    <button
                      key={ds.id}
                      type="button"
                      data-testid={`ai-wizard-dataset-${ds.name}`}
                      onClick={() => toggleDataset(ds.id)}
                      className={cn(
                        "flex items-center justify-between rounded border px-2.5 py-2 text-left cursor-pointer",
                        checked
                          ? "border-xblue bg-xblue-soft text-xblue"
                          : "border-border bg-background hover:bg-muted/50"
                      )}
                    >
                      <div className="min-w-0">
                        <div className="font-medium truncate">
                          {ds.display_name || ds.name}
                        </div>
                        <div className="font-mono-tech text-[10px] opacity-80 truncate">
                          {ds.name} · {ds.row_count} 行
                        </div>
                      </div>
                      <Badge variant={checked ? "xblue" : "outline"}>
                        {ds.status}
                      </Badge>
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          {/* Knowledge Documents Selection */}
          <div className="space-y-1.5">
            <span className="font-medium flex items-center gap-1.5">
              <FileText className="h-3.5 w-3.5 text-xblue" />
              2. 勾选已入库知识文档抽取超图实体关系 ({selectedDocIds.length}/
              {docNodes.length})
            </span>
            {docNodes.length === 0 ? (
              <div className="rounded border border-dashed border-border p-2.5 text-muted-foreground">
                暂无已入库超图文档节点。
              </div>
            ) : (
              <div className="flex flex-wrap gap-1.5 max-h-28 overflow-y-auto">
                {docNodes.map((doc) => {
                  const checked = selectedDocIds.includes(doc.id);
                  return (
                    <button
                      key={doc.id}
                      type="button"
                      onClick={() => toggleDoc(doc.id)}
                      className={cn(
                        "rounded border px-2.5 py-1 text-xs cursor-pointer",
                        checked
                          ? "border-xblue bg-xblue-soft text-xblue"
                          : "border-border bg-background hover:bg-muted/50"
                      )}
                    >
                      {doc.label}
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          {/* Natural Language Description */}
          <div>
            <label className="block font-medium mb-1">
              3. 自然语言业务领域描述 (description)
            </label>
            <Textarea
              data-testid="ai-draft-description-input"
              rows={3}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="例如：构建制造工厂 Factory、区域仓库 Warehouse 与客户订单 Order 三级履约拓扑，统计总需求量 total_demand 与库存满足率 fill_rate..."
              className="text-xs"
            />
          </div>
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
          <Button variant="outline" onClick={onClose} disabled={generating}>
            取消
          </Button>
          <Button
            data-testid="ai-draft-submit-btn"
            onClick={() => void handleSubmit()}
            disabled={generating}
          >
            <Sparkles className="h-3.5 w-3.5" />
            {generating ? "AI 分析与生成中..." : "生成模型草案并载入画布"}
          </Button>
        </div>
      </div>
    </div>
  );
}
