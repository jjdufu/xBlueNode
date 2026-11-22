import { useState } from "react";
import { FolderGit2, GitMerge, Layers, Plus, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type {
  CreateSubgraphRequest,
  NodeTypeSpec,
  SchemaDraft,
  SubgraphInfo,
} from "@/lib/api/xblue";
import { Badge, Button, Input, Modal } from "@/components/ui/primitives";
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
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

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
            display_name: tx("企业主图", "Main Graph"),
            description: tx("权威生产主图", "Production main graph"),
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
    ? `${activeDraft.title}${isDirty ? tx(" (未保存)", " (Unsaved)") : ""}`
    : isDirty
      ? `${draftTitle || tx("工作区草案", "Workspace Draft")}${tx(" (未保存)", " (Unsaved)")}`
      : tx("已发布快照", "Published Snapshot");

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
      setFormError(
        tx(
          "标识需以小写字母开头，仅含小写字母、数字、下划线或连字符",
          "Slug must start with lowercase letter and contain only a-z, 0-9, _, -"
        )
      );
      return;
    }
    if (trimmedSlug === "main") {
      setFormError(
        tx("main 为系统保留主图标识", "'main' is reserved for main graph")
      );
      return;
    }
    if (!trimmedDisplay) {
      setFormError(tx("子图名称为空", "Display name required"));
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
      setFormError(
        err instanceof Error
          ? err.message
          : tx("创建子图失败", "Failed to create subgraph")
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="rounded-md border border-border bg-card px-3 py-2 space-y-2">
      {/* Top Status Strip */}
      <div
        data-testid="studio-status-bar"
        className="flex flex-wrap items-center justify-between gap-2 text-xs border-b border-border pb-2"
      >
        <div className="flex flex-wrap items-center gap-1.5">
          <Layers className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="font-medium">
            {currentInfo?.is_main
              ? tx("企业主图", "Main Graph")
              : (currentInfo?.display_name ?? activeSubgraph)}
          </span>
          <Badge variant={activeSubgraph === "main" ? "xblue" : "neutral"}>
            {activeSubgraph}
          </Badge>
          <span className="text-muted-foreground">·</span>
          <span className="text-muted-foreground">
            {tx("当前草案:", "Draft:")}
          </span>
          <span className="font-medium" data-testid="studio-active-draft-label">
            {effectiveDraftLabel}
          </span>
          {activeDraft && <Badge variant="outline">{activeDraft.source}</Badge>}
          <span className="text-muted-foreground">·</span>
          <span
            className="font-mono-tech font-medium text-xblue"
            data-testid="studio-base-version"
          >
            {tx(`基线 v${baseVersion}`, `Base v${baseVersion}`)}
          </span>
          {publishedVersion !== baseVersion && (
            <Badge variant="outline">
              {tx(`最新 v${publishedVersion}`, `Latest v${publishedVersion}`)}
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          {canEdit && (
            <Button
              variant="outline"
              size="sm"
              data-testid="open-create-subgraph-btn"
              onClick={openModal}
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新建子图", "New Subgraph")}
            </Button>
          )}
          <Button
            variant={activeSubgraph !== "main" ? "primary" : "outline"}
            size="sm"
            data-testid="open-merge-dialog-btn"
            disabled={activeSubgraph === "main"}
            onClick={onOpenMergeDialog}
          >
            <GitMerge className="h-3.5 w-3.5" />
            {tx("合并至主图", "Merge to Main")}
          </Button>
        </div>
      </div>

      {/* Subgraph Switcher Tabs */}
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-[11px] text-muted-foreground flex items-center gap-1 mr-1">
          <FolderGit2 className="h-3 w-3" />
          {tx("子图空间:", "Subgraphs:")}
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
                <span>
                  {sg.is_main ? tx("企业主图", "Main Graph") : sg.display_name}
                </span>
                <span className="font-mono-tech text-[11px] opacity-80">
                  {sg.slug}
                </span>
                <span className="font-mono-tech text-[10px] rounded bg-muted px-1 py-0.2 text-foreground">
                  v{sg.current_version}
                </span>
              </button>
              {!sg.is_main && selected && onDeleteSubgraph && canEdit && (
                <button
                  type="button"
                  title={tx(`删除子图 ${sg.slug}`, `Delete ${sg.slug}`)}
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
      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        testId="create-subgraph-modal"
        maxWidthClass="max-w-md"
        title={tx("新建部门子图", "New Department Subgraph")}
        footer={
          <>
            <Button
              variant="outline"
              onClick={() => setCreateOpen(false)}
              disabled={submitting}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              data-testid="confirm-create-subgraph-btn"
              onClick={() => void handleSubmitCreate()}
              disabled={submitting}
            >
              {submitting
                ? tx("创建中…", "Creating…")
                : tx("创建子图", "Create")}
            </Button>
          </>
        }
      >
        {formError && (
          <div className="rounded border border-red-500/30 bg-red-500/10 px-2.5 py-1.5 text-xs text-red-600 dark:text-red-400">
            {formError}
          </div>
        )}

        <div className="space-y-2.5 text-xs">
          <div>
            <label className="block font-medium mb-1">
              {tx("子图标识", "Subgraph Slug")}
            </label>
            <Input
              data-testid="create-subgraph-slug"
              placeholder="logistics"
              value={slug}
              onChange={(e) => setSlug(e.target.value)}
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block font-medium mb-1">
                {tx("显示名称", "Display Name")}
              </label>
              <Input
                data-testid="create-subgraph-display-name"
                placeholder={tx("物流履约子图", "Logistics Subgraph")}
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("归属部门", "Department")}
              </label>
              <Input
                data-testid="create-subgraph-dept"
                placeholder="logistics"
                value={ownerDept}
                onChange={(e) => setOwnerDept(e.target.value)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div>
            <label className="block font-medium mb-1">
              {tx("业务说明", "Description")}
            </label>
            <Input
              placeholder={tx("物流干线与仓配模型", "Logistics domain model")}
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
                {tx("继承主图基线节点类型", "Seed node types from Main Graph")}
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
      </Modal>
    </div>
  );
}
