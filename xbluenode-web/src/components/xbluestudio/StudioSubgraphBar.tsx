import { useState } from "react";
import {
  FolderGit2,
  GitMerge,
  Layers,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import type {
  CreateSubgraphRequest,
  NodeTypeSpec,
  SchemaDraft,
  SubgraphInfo,
} from "@/lib/api/xblue";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioSubgraphBarProps {
  subgraphs: SubgraphInfo[];
  activeSubgraph: string;
  activeDraft: SchemaDraft | null;
  draftTitle: string;
  baseVersion: number;
  publishedVersion: number;
  isDirty: boolean;
  mainNodeTypes: NodeTypeSpec[];
  onSelectSubgraph: (slug: string) => void;
  onCreateSubgraph: (req: CreateSubgraphRequest) => Promise<void>;
  onDeleteSubgraph?: (slug: string) => Promise<void>;
  onOpenMergeDialog: () => void;
}

const SLUG_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;

export function StudioSubgraphBar({
  subgraphs,
  activeSubgraph,
  activeDraft,
  draftTitle,
  baseVersion,
  publishedVersion,
  isDirty,
  mainNodeTypes,
  onSelectSubgraph,
  onCreateSubgraph,
  onDeleteSubgraph,
  onOpenMergeDialog,
}: StudioSubgraphBarProps) {
  const [createOpen, setCreateOpen] = useState(false);
  const [slug, setSlug] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [ownerDept, setOwnerDept] = useState("default");
  const [description, setDescription] = useState("");
  const [seedFromMain, setSeedFromMain] = useState(true);
  const [selectedSeedTypes, setSelectedSeedTypes] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const normalizedList: SubgraphInfo[] =
    subgraphs.length > 0
      ? subgraphs
      : [
          {
            id: "00000000-0000-0000-0000-000000000000",
            slug: "main",
            display_name: "企业主图",
            description: "企业权威生产主图",
            owner_dept: "default",
            is_main: true,
            current_version: publishedVersion,
            created_at: "",
            updated_at: "",
          },
        ];

  const currentInfo =
    normalizedList.find((s) => s.slug === activeSubgraph) ?? normalizedList[0];

  const effectiveDraftLabel = activeDraft
    ? `${activeDraft.title}${isDirty ? " (未保存修改)" : ""}`
    : isDirty
      ? `${draftTitle || "新建工作区草案"} (未保存)`
      : "已发布快照浏览";

  const openModal = () => {
    setSlug("");
    setDisplayName("");
    setOwnerDept("default");
    setDescription("");
    setSeedFromMain(mainNodeTypes.length > 0);
    setSelectedSeedTypes(mainNodeTypes.map((nt) => nt.name));
    setFormError(null);
    setCreateOpen(true);
  };

  const toggleSeedType = (typeName: string) => {
    setSelectedSeedTypes((prev) =>
      prev.includes(typeName)
        ? prev.filter((item) => item !== typeName)
        : [...prev, typeName]
    );
  };

  const handleSubmitCreate = async () => {
    const trimmedSlug = slug.trim();
    const trimmedDisplay = displayName.trim();
    if (!SLUG_PATTERN.test(trimmedSlug)) {
      setFormError("子图标识需以小写字母开头，仅含小写字母、数字、下划线或连字符");
      return;
    }
    if (trimmedSlug === "main") {
      setFormError("main 为系统保留企业主图标识，请使用部门子图名称");
      return;
    }
    if (!trimmedDisplay) {
      setFormError("请输入部门子图中文名称");
      return;
    }
    setSubmitting(true);
    setFormError(null);
    try {
      await onCreateSubgraph({
        slug: trimmedSlug,
        display_name: trimmedDisplay,
        description: description.trim(),
        owner_dept: ownerDept.trim() || "default",
        seed_from_main: seedFromMain,
        seed_node_types: seedFromMain ? selectedSeedTypes : undefined,
      });
      setCreateOpen(false);
    } catch (err) {
      setFormError(err instanceof Error ? err.message : "创建子图失败");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="rounded-md border border-border bg-card px-3 py-2 space-y-2">
      {/* Top Status Strip: 当前子图 · 当前编辑草案 · 基于版本 vN */}
      <div
        data-testid="studio-status-bar"
        className="flex flex-wrap items-center justify-between gap-2 text-xs border-b border-border pb-2"
      >
        <div className="flex flex-wrap items-center gap-1.5">
          <Layers className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="font-medium">
            当前子图：{currentInfo?.display_name ?? activeSubgraph}
          </span>
          <Badge variant={activeSubgraph === "main" ? "xblue" : "neutral"}>
            {activeSubgraph}
          </Badge>
          <span className="text-muted-foreground">·</span>
          <span className="text-muted-foreground">当前编辑草案：</span>
          <span className="font-medium" data-testid="studio-active-draft-label">
            {effectiveDraftLabel}
          </span>
          {activeDraft && (
            <Badge variant="outline">{activeDraft.source}</Badge>
          )}
          <span className="text-muted-foreground">·</span>
          <span className="font-mono-tech font-medium text-xblue" data-testid="studio-base-version">
            基于版本 v{baseVersion}
          </span>
          {publishedVersion !== baseVersion && (
            <Badge variant="outline">最新已发布 v{publishedVersion}</Badge>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          <Button
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="open-create-subgraph-btn"
            onClick={openModal}
          >
            <Plus className="h-3.5 w-3.5" />
            新建部门子图
          </Button>
          <Button
            variant={activeSubgraph !== "main" ? "primary" : "outline"}
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="open-merge-dialog-btn"
            disabled={activeSubgraph === "main"}
            onClick={onOpenMergeDialog}
            title={
              activeSubgraph === "main"
                ? "切换到部门子图后可与企业主图对比并合并"
                : "预览跨子图同义实体对齐并合并到企业主图 main"
            }
          >
            <GitMerge className="h-3.5 w-3.5" />
            对比并合并到主图
          </Button>
        </div>
      </div>

      {/* Subgraph Switcher Tabs */}
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-[11px] text-muted-foreground flex items-center gap-1 mr-1">
          <FolderGit2 className="h-3 w-3" />
          多子图空间:
        </span>
        {normalizedList.map((sg) => {
          const selected = sg.slug === activeSubgraph;
          return (
            <div key={sg.slug} className="inline-flex items-center">
              <button
                type="button"
                data-testid={`subgraph-tab-${sg.slug}`}
                onClick={() => onSelectSubgraph(sg.slug)}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded px-2.5 py-1 text-xs font-medium transition-colors cursor-pointer border",
                  selected
                    ? "bg-xblue-soft text-xblue border-xblue/40"
                    : "bg-background text-muted-foreground border-border hover:text-foreground hover:bg-muted"
                )}
              >
                <span>{sg.is_main ? "企业主图" : sg.display_name}</span>
                <span className="font-mono-tech text-[11px] opacity-80">
                  {sg.slug}
                </span>
                <span className="font-mono-tech text-[10px] rounded bg-muted px-1 py-0.2 text-foreground">
                  v{sg.current_version}
                </span>
              </button>
              {!sg.is_main && selected && onDeleteSubgraph && (
                <button
                  type="button"
                  title={`删除子图 ${sg.slug}`}
                  onClick={() => void onDeleteSubgraph(sg.slug)}
                  className="ml-0.5 p-1 rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                >
                  <Trash2 className="h-3 w-3" />
                </button>
              )}
            </div>
          );
        })}
      </div>

      {/* Create Department Subgraph Modal */}
      {createOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
          data-testid="create-subgraph-modal"
        >
          <div className="w-full max-w-md rounded-md border border-border bg-card p-4 shadow-lg space-y-3">
            <div className="flex items-center justify-between border-b border-border pb-2">
              <div>
                <h3 className="text-sm font-semibold">新建部门隔离建模子图</h3>
                <p className="text-[11px] text-muted-foreground">
                  部门子图独立演进版本与映射预览，成熟后可对齐合并入企业主图 main
                </p>
              </div>
              <button
                type="button"
                onClick={() => setCreateOpen(false)}
                className="p-1 rounded hover:bg-muted text-muted-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            {formError && (
              <div className="rounded border border-red-500/30 bg-red-500/10 px-2.5 py-1.5 text-xs text-red-600 dark:text-red-400">
                {formError}
              </div>
            )}

            <div className="space-y-2.5 text-xs">
              <div>
                <label className="block font-medium mb-1">
                  子图英文标识 (slug)
                </label>
                <Input
                  data-testid="create-subgraph-slug"
                  placeholder="例如 logistics、finance、manufacturing"
                  value={slug}
                  onChange={(e) => setSlug(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block font-medium mb-1">子图中文名称</label>
                  <Input
                    data-testid="create-subgraph-display-name"
                    placeholder="例如 物流履约子图"
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
                <div>
                  <label className="block font-medium mb-1">归属部门 ID</label>
                  <Input
                    data-testid="create-subgraph-dept"
                    placeholder="例如 logistics"
                    value={ownerDept}
                    onChange={(e) => setOwnerDept(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              </div>

              <div>
                <label className="block font-medium mb-1">业务说明</label>
                <Input
                  placeholder="可选，描述本部门建模范围"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>

              <div className="rounded border border-border bg-muted/40 p-2.5 space-y-2">
                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={seedFromMain}
                    onChange={(e) => setSeedFromMain(e.target.checked)}
                    className="rounded border-border"
                  />
                  <span className="font-medium">
                    从企业主图 (main) 拷贝初始节点类型作为建模上下文
                  </span>
                </label>

                {seedFromMain && mainNodeTypes.length > 0 && (
                  <div className="flex flex-wrap gap-1.5 pt-1">
                    {mainNodeTypes.map((nt) => {
                      const checked = selectedSeedTypes.includes(nt.name);
                      return (
                        <button
                          key={nt.name}
                          type="button"
                          onClick={() => toggleSeedType(nt.name)}
                          className={cn(
                            "rounded border px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer",
                            checked
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border bg-background text-muted-foreground"
                          )}
                        >
                          {nt.display_name} ({nt.name})
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>

            <div className="flex items-center justify-end gap-2 pt-2 border-t border-border">
              <Button
                variant="outline"
                onClick={() => setCreateOpen(false)}
                disabled={submitting}
              >
                取消
              </Button>
              <Button
                data-testid="confirm-create-subgraph-btn"
                onClick={() => void handleSubmitCreate()}
                disabled={submitting}
              >
                {submitting ? "创建中..." : "创建子图"}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
