import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, GitMerge, RefreshCw } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { SubgraphMergeStrategy } from "@/lib/api/enums";
import type {
  MergePreview,
  MergeSubgraphRequest,
  TypeMergeResolution,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  Modal,
  Select,
} from "@/components/ui/primitives";

export interface SubgraphMergeDialogProps {
  open: boolean;
  subgraphSlug: string;
  subgraphDisplayName: string;
  preview: MergePreview | null;
  loading: boolean;
  merging: boolean;
  error: string | null;
  onClose: () => void;
  onRefreshPreview: (resolutions: TypeMergeResolution[]) => Promise<void>;
  onExecuteMerge: (req: MergeSubgraphRequest) => Promise<void>;
}

const STRATEGY_LABELS: Record<
  SubgraphMergeStrategy,
  { zh: string; en: string }
> = {
  REUSE_MAIN: {
    zh: "复用主图类型",
    en: "Reuse Main Type",
  },
  EXTEND_PROPS: {
    zh: "合并扩展属性与规则",
    en: "Extend Properties & Rules",
  },
  BRIDGE_LINK: {
    zh: "保留独立类型并建 1:1 桥接边",
    en: "Keep Distinct & Add 1:1 Bridge",
  },
};

const CONFLICT_KIND_LABELS: Record<string, { zh: string; en: string }> = {
  PROPERTY_TYPE_CONFLICT: {
    zh: "属性类型冲突",
    en: "Property Type Conflict",
  },
  RULE_CONFLICT: {
    zh: "指标规则冲突",
    en: "Metric Rule Conflict",
  },
  EDGE_CONFLICT: {
    zh: "关系端点冲突",
    en: "Relation Endpoint Conflict",
  },
  MAPPING_NAME_CONFLICT: {
    zh: "映射重名冲突",
    en: "Mapping Name Conflict",
  },
};

export function SubgraphMergeDialog({
  open,
  subgraphSlug,
  subgraphDisplayName,
  preview,
  loading,
  merging,
  error,
  onClose,
  onRefreshPreview,
  onExecuteMerge,
}: SubgraphMergeDialogProps) {
  const { isZh, tx } = useI18n();
  const { can } = usePrincipal();
  const canPublish = can("STUDIO_PUBLISH");

  const [defaultStrategy, setDefaultStrategy] =
    useState<SubgraphMergeStrategy>("EXTEND_PROPS");
  const [resolutions, setResolutions] = useState<
    Record<string, TypeMergeResolution>
  >({});
  const [changeSummary, setChangeSummary] = useState("");
  const [forceMerge, setForceMerge] = useState(false);

  useEffect(() => {
    if (!preview) return;
    const next: Record<string, TypeMergeResolution> = {};
    for (const item of preview.alignments) {
      next[item.sub_type] = {
        sub_type: item.sub_type,
        main_type: item.main_type,
        strategy: item.suggested_strategy,
        bridge_edge_name:
          item.suggested_strategy === "BRIDGE_LINK"
            ? "SAME_ENTITY_AS"
            : undefined,
      };
    }
    setResolutions(next);
    if (!changeSummary) {
      setChangeSummary(
        tx(
          `合并部门子图 ${subgraphSlug} 至企业主图`,
          `Merge subgraph ${subgraphSlug} into main`
        )
      );
    }
  }, [preview, subgraphSlug, changeSummary, tx]);

  const resolutionList = Object.values(resolutions);

  const updateResolutionStrategy = (
    subType: string,
    mainType: string,
    strategy: SubgraphMergeStrategy
  ) => {
    setResolutions((prev) => ({
      ...prev,
      [subType]: {
        sub_type: subType,
        main_type: mainType,
        strategy,
        bridge_edge_name:
          strategy === "BRIDGE_LINK"
            ? prev[subType]?.bridge_edge_name || "SAME_ENTITY_AS"
            : undefined,
      },
    }));
  };

  const updateBridgeEdge = (subType: string, edgeName: string) => {
    setResolutions((prev) => {
      const existing = prev[subType];
      if (!existing) return prev;
      return {
        ...prev,
        [subType]: {
          ...existing,
          bridge_edge_name: edgeName,
        },
      };
    });
  };

  const handleMerge = async () => {
    if (!canPublish) return;
    await onExecuteMerge({
      base_main_version: preview?.main_version,
      default_strategy: defaultStrategy,
      resolutions: resolutionList,
      change_summary:
        changeSummary.trim() ||
        tx(`合并子图 ${subgraphSlug}`, `Merge ${subgraphSlug}`),
      force: forceMerge,
    });
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="subgraph-merge-dialog"
      maxWidthClass="max-w-3xl"
      title={
        <>
          <GitMerge className="h-4 w-4 text-xblue shrink-0" />
          <span className="truncate">
            {tx("子图对齐与合并", "Subgraph Alignment & Merge")} ·{" "}
            {subgraphDisplayName} ({subgraphSlug}) → main
          </span>
        </>
      }
      footer={
        <>
          <Button variant="outline" onClick={onClose} disabled={merging}>
            {tx("取消", "Cancel")}
          </Button>
          <Button
            data-testid="execute-subgraph-merge-btn"
            onClick={() => void handleMerge()}
            disabled={!canPublish || loading || merging || !preview}
          >
            <GitMerge className="h-3.5 w-3.5" />
            {merging
              ? tx("合并中…", "Merging…")
              : tx("确认合并", "Confirm Merge")}
          </Button>
        </>
      }
    >
      {error && (
        <div className="rounded border border-red-500/30 bg-red-500/10 p-2.5 text-red-600 dark:text-red-400">
          {error}
        </div>
      )}

      {loading ? (
        <div className="py-10 text-center text-muted-foreground">
          {tx("计算实体对齐与冲突中…", "Computing alignments and conflicts…")}
        </div>
      ) : preview ? (
        <div className="space-y-4">
          <div className="flex items-center gap-2 text-[11px] font-mono-tech text-muted-foreground">
            <Badge variant="outline">
              {subgraphSlug} v{preview.subgraph_version}
            </Badge>
            <span>→</span>
            <Badge variant="xblue">main v{preview.main_version}</Badge>
          </div>

          {/* Net-New Additions Summary */}
          <div className="grid grid-cols-3 gap-2">
            <div className="rounded border border-border bg-muted/30 p-2.5">
              <div className="text-[11px] text-muted-foreground">
                {tx("新增节点类型", "Added Node Types")} (
                {preview.added_node_types.length})
              </div>
              <div className="mt-1 flex flex-wrap gap-1">
                {preview.added_node_types.length > 0 ? (
                  preview.added_node_types.map((name) => (
                    <Badge key={name} variant="xblue">
                      + {name}
                    </Badge>
                  ))
                ) : (
                  <span className="text-muted-foreground">
                    {tx("无", "None")}
                  </span>
                )}
              </div>
            </div>
            <div className="rounded border border-border bg-muted/30 p-2.5">
              <div className="text-[11px] text-muted-foreground">
                {tx("新增关系类型", "Added Relations")} (
                {preview.added_edge_types.length})
              </div>
              <div className="mt-1 flex flex-wrap gap-1">
                {preview.added_edge_types.length > 0 ? (
                  preview.added_edge_types.map((name) => (
                    <Badge key={name} variant="neutral">
                      + {name}
                    </Badge>
                  ))
                ) : (
                  <span className="text-muted-foreground">
                    {tx("无", "None")}
                  </span>
                )}
              </div>
            </div>
            <div className="rounded border border-border bg-muted/30 p-2.5">
              <div className="text-[11px] text-muted-foreground">
                {tx("新增映射规则", "Added Mappings")} (
                {preview.added_mappings.length})
              </div>
              <div className="mt-1 flex flex-wrap gap-1">
                {preview.added_mappings.length > 0 ? (
                  preview.added_mappings.map((name) => (
                    <Badge key={name} variant="outline">
                      + {name}
                    </Badge>
                  ))
                ) : (
                  <span className="text-muted-foreground">
                    {tx("无", "None")}
                  </span>
                )}
              </div>
            </div>
          </div>

          {/* Synonymous Entity Alignments */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <h3 className="font-semibold text-xs">
                {tx("同义实体对齐", "Entity Alignments")} (
                {preview.alignments.length})
              </h3>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void onRefreshPreview(resolutionList)}
              >
                <RefreshCw className="h-3 w-3" />
                {tx("重新预演", "Re-preview")}
              </Button>
            </div>

            {preview.alignments.length === 0 ? (
              <div className="rounded border border-border p-3 text-muted-foreground flex items-center gap-2">
                <CheckCircle2 className="h-4 w-4 text-emerald-600" />
                {tx("无重叠同义实体，全部类型直接并入主图", "No overlapping entities")}
              </div>
            ) : (
              <div className="rounded border border-border overflow-hidden">
                <table className="w-full text-left border-collapse">
                  <thead className="bg-muted/60 text-[11px] text-muted-foreground">
                    <tr>
                      <th className="px-3 py-2 font-medium">
                        {tx("子图类型", "Subgraph Type")}
                      </th>
                      <th className="px-3 py-2 font-medium">
                        {tx("主图目标", "Main Target")}
                      </th>
                      <th className="px-3 py-2 font-medium">
                        {tx("相似度", "Score")}
                      </th>
                      <th className="px-3 py-2 font-medium">
                        {tx("合并策略", "Strategy")}
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {preview.alignments.map((al) => {
                      const current = resolutions[al.sub_type];
                      const chosen =
                        current?.strategy ?? al.suggested_strategy;
                      const pct = Math.round(al.score * 100);
                      return (
                        <tr key={`${al.sub_type}->${al.main_type}`}>
                          <td className="px-3 py-2 font-mono-tech font-medium">
                            {al.sub_type}
                          </td>
                          <td className="px-3 py-2 font-mono-tech">
                            {al.main_type}
                          </td>
                          <td className="px-3 py-2">
                            <div className="flex items-center gap-2">
                              <div className="h-1.5 w-16 rounded bg-muted overflow-hidden">
                                <div
                                  className="h-full bg-xblue"
                                  style={{ width: `${Math.min(100, pct)}%` }}
                                />
                              </div>
                              <Badge variant="xblue">
                                {al.score.toFixed(2)} ({pct}%)
                              </Badge>
                            </div>
                          </td>
                          <td className="px-3 py-2 space-y-1.5">
                            <Select
                              value={chosen}
                              onChange={(e) =>
                                updateResolutionStrategy(
                                  al.sub_type,
                                  al.main_type,
                                  e.target.value as SubgraphMergeStrategy
                                )
                              }
                              className="h-8 w-full"
                            >
                              {(
                                Object.keys(
                                  STRATEGY_LABELS
                                ) as SubgraphMergeStrategy[]
                              ).map((st) => (
                                <option key={st} value={st}>
                                  {isZh
                                    ? STRATEGY_LABELS[st].zh
                                    : STRATEGY_LABELS[st].en}
                                </option>
                              ))}
                            </Select>
                            {chosen === "BRIDGE_LINK" && (
                              <Input
                                value={
                                  current?.bridge_edge_name ?? "SAME_ENTITY_AS"
                                }
                                onChange={(e) =>
                                  updateBridgeEdge(al.sub_type, e.target.value)
                                }
                                placeholder="SAME_ENTITY_AS"
                                className="h-7 text-xs font-mono-tech"
                              />
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {/* Conflicts Section */}
          <div className="space-y-2">
            <h3 className="font-semibold text-xs">
              {tx("冲突清单", "Conflicts")} ({preview.conflicts.length})
            </h3>
            {preview.conflicts.length === 0 ? (
              <div className="rounded border border-border p-2.5 text-muted-foreground">
                {tx("0 项结构冲突", "0 structural conflicts")}
              </div>
            ) : (
              <div className="space-y-1.5">
                {preview.conflicts.map((c, idx) => {
                  const kindMeta = CONFLICT_KIND_LABELS[c.kind];
                  return (
                    <div
                      key={`${c.kind}-${c.sub_item}-${idx}`}
                      className="rounded border border-amber-500/40 bg-amber-500/10 p-2.5 flex items-start gap-2"
                    >
                      <AlertTriangle className="h-4 w-4 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                      <div className="space-y-0.5">
                        <div className="flex items-center gap-2">
                          <Badge variant="warning">
                            {kindMeta
                              ? isZh
                                ? kindMeta.zh
                                : kindMeta.en
                              : c.kind}
                          </Badge>
                          <span className="font-mono-tech font-medium">
                            {c.sub_item} ↔ {c.main_item}
                          </span>
                        </div>
                        <p className="text-muted-foreground">{c.detail}</p>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {/* Default Strategy & Commit Summary */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 pt-2 border-t border-border">
            <div>
              <label className="block font-medium mb-1">
                {tx("默认对齐策略", "Default Strategy")}
              </label>
              <Select
                data-testid="merge-default-strategy"
                value={defaultStrategy}
                onChange={(e) =>
                  setDefaultStrategy(e.target.value as SubgraphMergeStrategy)
                }
                className="h-8 w-full"
              >
                {(
                  Object.keys(STRATEGY_LABELS) as SubgraphMergeStrategy[]
                ).map((st) => (
                  <option key={st} value={st}>
                    {isZh ? STRATEGY_LABELS[st].zh : STRATEGY_LABELS[st].en}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("发布摘要", "Change Summary")}
              </label>
              <Input
                data-testid="merge-change-summary"
                value={changeSummary}
                onChange={(e) => setChangeSummary(e.target.value)}
                placeholder={tx("合并物流子图至主图", "Merge logistics subgraph")}
                className="h-8 text-xs"
              />
            </div>
          </div>

          {preview.conflicts.length > 0 && (
            <label className="flex items-center gap-2 text-xs cursor-pointer">
              <input
                type="checkbox"
                checked={forceMerge}
                onChange={(e) => setForceMerge(e.target.checked)}
              />
              <span>
                {tx(
                  "强制合并覆盖破坏性冲突",
                  "Force merge despite breaking conflicts"
                )}
              </span>
            </label>
          )}
        </div>
      ) : null}
    </Modal>
  );
}
