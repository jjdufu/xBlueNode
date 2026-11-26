import { useState } from "react";
import {
  FlaskConical,
  FolderGit2,
  GitMerge,
  Plus,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type {
  CreateSubgraphRequest,
  NodeTypeSpec,
  SchemaDraft,
  SubgraphInfo,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  ConfirmDialog,
  Input,
  Modal,
  Select,
  Switch,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioSubgraphBarProps {
  subgraphs: SubgraphInfo[];
  activeSubgraph: string;
  activeDraft: SchemaDraft | null;
  drafts?: SchemaDraft[];
  draftTitle: string;
  baseVersion: number;
  publishedVersion: number;
  runtimeNodeCount?: number;
  isDirty: boolean;
  busyOp?: string | null;
  mainNodeTypes: NodeTypeSpec[];
  onSelectSubgraph: (slug: string) => void;
  onSelectDraft?: (draft: SchemaDraft) => void;
  onDiscardDraft?: () => void;
  onCreateSubgraph: (req: CreateSubgraphRequest) => Promise<void>;
  onDeleteSubgraph?: (slug: string) => Promise<void>;
  onDryRunSubgraph?: () => Promise<void>;
  onResetSubgraphNodes?: () => Promise<void>;
  onOpenMergeDialog: () => void;
}

const SLUG_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;

export function StudioSubgraphBar({
  subgraphs,
  activeSubgraph,
  activeDraft,
  drafts = [],
  draftTitle,
  baseVersion,
  publishedVersion,
  runtimeNodeCount = 0,
  isDirty,
  busyOp = null,
  mainNodeTypes,
  onSelectSubgraph,
  onSelectDraft,
  onDiscardDraft,
  onCreateSubgraph,
  onDeleteSubgraph,
  onDryRunSubgraph,
  onResetSubgraphNodes,
  onOpenMergeDialog,
}: StudioSubgraphBarProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [createOpen, setCreateOpen] = useState(false);
  const [slug, setSlug] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [ownerDept, setOwnerDept] = useState("default");
  const [description, setDescription] = useState("");
  const [isDevSandbox, setIsDevSandbox] = useState(false);
  const [seedFromMain, setSeedFromMain] = useState(true);
  const [selectedSeedTypes, setSelectedSeedTypes] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const [resetConfirmOpen, setResetConfirmOpen] = useState(false);
  const [resetting, setResetting] = useState(false);

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

  const activeSgInfo = normalizedList.find((sg) => sg.slug === activeSubgraph);
  const isActiveSandbox = Boolean(activeSgInfo?.is_dev_sandbox);

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
    setIsDevSandbox(false);
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
        is_dev_sandbox: isDevSandbox,
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

  const handleConfirmReset = async () => {
    if (!onResetSubgraphNodes) {
      setResetConfirmOpen(false);
      return;
    }
    setResetting(true);
    try {
      await onResetSubgraphNodes();
      setResetConfirmOpen(false);
    } finally {
      setResetting(false);
    }
  };

  return (
    <div
      data-testid="studio-status-bar"
      className="shrink-0 rounded-md border border-border bg-card px-3 py-1.5 flex flex-wrap items-center justify-between gap-2 text-xs"
    >
      {/* Left: Single-Row Subgraph Switcher + Active Draft & Base Version (Zero duplication) */}
      <div className="flex flex-wrap items-center gap-1.5 min-w-0">
        <FolderGit2 className="h-3.5 w-3.5 text-xblue shrink-0" />
        {normalizedList.map((sg) => {
          const selected = sg.slug === activeSubgraph;
          return (
            <div key={sg.slug} className="inline-flex items-center">
              <button
                type="button"
                data-testid={`subgraph-tab-${sg.slug}`}
                onClick={() => onSelectSubgraph(sg.slug)}
                title={`${sg.is_main ? tx("企业主图", "Main Graph") : sg.display_name} (${sg.slug} · v${sg.current_version})`}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium transition-colors cursor-pointer",
                  selected
                    ? "bg-xblue-soft text-xblue"
                    : "text-muted-foreground hover:text-foreground hover:bg-muted/70"
                )}
              >
                <span>
                  {sg.is_main ? tx("企业主图", "Main Graph") : sg.display_name}
                </span>
                {selected && (
                  <span className="font-mono-tech text-[10px] opacity-75">
                    v{sg.current_version}
                  </span>
                )}
              </button>
            </div>
          );
        })}

        {isActiveSandbox && (
          <>
            <Badge variant="xblue" data-testid="studio-dev-sandbox-badge">
              {tx("开发沙盒", "Dev Sandbox")}
            </Badge>
            <Badge variant="outline" data-testid="studio-sandbox-node-count-badge">
              {tx("节点", "Nodes")} {fmtNum(runtimeNodeCount, 0)}
            </Badge>
          </>
        )}

        <span className="text-border px-0.5">|</span>

        <span className="text-muted-foreground">
          {tx("草案:", "Draft:")}
        </span>
        {drafts.length > 1 && onSelectDraft ? (
          <Select
            value={activeDraft?.id ?? ""}
            onChange={(e) => {
              const target = drafts.find((d) => d.id === e.target.value);
              if (target) onSelectDraft(target);
            }}
            className="h-7 w-48 text-xs"
          >
            {drafts.map((d) => (
              <option key={d.id} value={d.id}>
                {d.title} (v{d.base_version})
              </option>
            ))}
          </Select>
        ) : null}
        <span
          className={cn(
            "font-medium truncate max-w-[220px]",
            drafts.length > 1 && onSelectDraft && "sr-only"
          )}
          data-testid="studio-active-draft-label"
        >
          {effectiveDraftLabel}
        </span>

        <span className="text-border">·</span>
        <span
          className="font-mono-tech font-medium text-xblue"
          data-testid="studio-base-version"
        >
          {tx(`基线 v${baseVersion}`, `Base v${baseVersion}`)}
        </span>
        {publishedVersion !== baseVersion && (
          <Badge variant="warning">
            {tx(`最新 v${publishedVersion}`, `Latest v${publishedVersion}`)}
          </Badge>
        )}

        {activeDraft?.id && canEdit && onDiscardDraft && (
          <Button
            variant="ghost"
            size="sm"
            disabled={busyOp !== null}
            onClick={onDiscardDraft}
            className="h-7 px-2 text-red-600 hover:bg-red-500/10 dark:text-red-400"
          >
            <Trash2 className="h-3 w-3" />
            {tx("丢弃草案", "Discard")}
          </Button>
        )}
      </div>

      {/* Right: Subgraph Actions */}
      <div className="flex flex-wrap items-center gap-1.5 shrink-0">
        {isActiveSandbox && canEdit && (
          <>
            <Button
              variant="outline"
              size="sm"
              data-testid="studio-sandbox-dry-run-btn"
              disabled={busyOp !== null}
              onClick={() => {
                if (onDryRunSubgraph) void onDryRunSubgraph();
              }}
            >
              <FlaskConical className="h-3.5 w-3.5 text-xblue" />
              {busyOp === "dry-run-subgraph"
                ? tx("试灌中…", "Running…")
                : tx("试灌数据", "Dry Run")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              data-testid="studio-sandbox-reset-btn"
              disabled={busyOp !== null}
              onClick={() => setResetConfirmOpen(true)}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              {busyOp === "reset-subgraph"
                ? tx("清空中…", "Resetting…")
                : tx("清空节点", "Reset Nodes")}
            </Button>
          </>
        )}
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
          variant="outline"
          size="sm"
          data-testid="open-merge-dialog-btn"
          disabled={activeSubgraph === "main"}
          onClick={onOpenMergeDialog}
          className={cn(
            activeSubgraph !== "main" &&
              "border-xblue/60 text-xblue hover:bg-xblue-soft/60"
          )}
        >
          <GitMerge className="h-3.5 w-3.5" />
          {tx("合并至主图", "Merge to Main")}
        </Button>
        {activeSubgraph !== "main" && onDeleteSubgraph && canEdit && (
          <span className="ml-0.5 pl-1.5 border-l border-border inline-flex">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              title={tx(`删除子图 ${activeSubgraph}`, `Delete ${activeSubgraph}`)}
              onClick={() => void onDeleteSubgraph(activeSubgraph)}
              className="text-muted-foreground hover:text-red-500 hover:bg-red-500/10"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </span>
        )}
      </div>

      {/* Confirm Reset Sandbox Runtime Nodes Dialog */}
      <ConfirmDialog
        open={resetConfirmOpen}
        onClose={() => {
          if (!resetting) setResetConfirmOpen(false);
        }}
        onConfirm={() => void handleConfirmReset()}
        title={tx("清空节点", "Reset Nodes")}
        description={tx(
          "将清空当前开发沙盒子图内的全部运行态节点、关系与时序点，模型草案与主图数据不受影响。",
          "All runtime nodes, links, and time-series points in this dev sandbox subgraph will be deleted. Schema drafts and main graph data remain intact."
        )}
        confirmLabel={
          resetting ? tx("清空中…", "Resetting…") : tx("清空", "Reset")
        }
        danger
        loading={resetting}
      />

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

          <div className="flex items-center justify-between gap-2 rounded-md border border-border bg-muted/40 p-2.5">
            <span className="font-medium">
              {tx("开发沙盒", "Dev Sandbox")}
            </span>
            <Switch
              data-testid="new-subgraph-sandbox-switch"
              checked={isDevSandbox}
              onCheckedChange={setIsDevSandbox}
              aria-label={tx("开发沙盒", "Dev Sandbox")}
            />
          </div>

          <div className="rounded-md border border-border bg-muted/40 p-2.5 space-y-2">
            <div className="flex items-center justify-between gap-2">
              <span className="font-medium">
                {tx("继承主图基线节点类型", "Seed node types from Main Graph")}
              </span>
              <Switch
                checked={seedFromMain}
                onCheckedChange={setSeedFromMain}
                aria-label={tx("继承主图基线节点类型", "Seed node types")}
              />
            </div>

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
