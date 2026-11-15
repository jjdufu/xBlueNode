import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, GitMerge, RefreshCw, X } from "lucide-react";
import type { SubgraphMergeStrategy } from "@/lib/api/enums";
import type {
  MergePreview,
  MergeSubgraphRequest,
  TypeMergeResolution,
} from "@/lib/api/xblue";
import { Badge, Button, Input } from "@/components/ui/primitives";

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

const STRATEGY_LABELS: Record<SubgraphMergeStrategy, string> = {
  REUSE_MAIN: "REUSE_MAIN · 直接复用主图类型",
  EXTEND_PROPS: "EXTEND_PROPS · 扩展合并新增属性与规则",
  BRIDGE_LINK: "BRIDGE_LINK · 保留独立类型并建立 1:1 桥接边",
};

const CONFLICT_KIND_LABELS: Record<string, string> = {
  PROPERTY_TYPE_CONFLICT: "属性类型冲突",
  RULE_CONFLICT: "指标规则冲突",
  EDGE_CONFLICT: "关系基数/端点冲突",
  MAPPING_NAME_CONFLICT: "映射重名冲突",
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
      setChangeSummary(`合并部门子图 ${subgraphSlug} 至企业主图`);
    }
  }, [preview, subgraphSlug, changeSummary]);

  if (!open) return null;

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
    await onExecuteMerge({
      base_main_version: preview?.main_version,
      default_strategy: defaultStrategy,
      resolutions: resolutionList,
      change_summary:
        changeSummary.trim() || `合并子图 ${subgraphSlug} 至主图`,
      force: forceMerge,
    });
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      data-testid="subgraph-merge-dialog"
    >
      <div className="w-full max-w-3xl max-h-[88vh] flex flex-col rounded-md border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <GitMerge className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">
                跨子图本体对齐与合并 · {subgraphDisplayName} ({subgraphSlug}) → 企业主图 (main)
              </h2>
              {preview && (
                <p className="text-[11px] text-muted-foreground font-mono-tech">
                  子图已发布版本 v{preview.subgraph_version} · 企业主图基线版本 v
                  {preview.main_version}
                </p>
              )}
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

          {loading ? (
            <div className="py-10 text-center text-muted-foreground">
              正在计算同义实体对齐得分与属性/规则冲突...
            </div>
          ) : preview ? (
            <>
              {/* Net-New Additions Summary */}
              <div className="grid grid-cols-3 gap-2">
                <div className="rounded border border-border bg-muted/30 p-2.5">
                  <div className="text-[11px] text-muted-foreground">
                    净新增节点类型 ({preview.added_node_types.length})
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {preview.added_node_types.length > 0 ? (
                      preview.added_node_types.map((name) => (
                        <Badge key={name} variant="xblue">
                          + {name}
                        </Badge>
                      ))
                    ) : (
                      <span className="text-muted-foreground">无</span>
                    )}
                  </div>
                </div>
                <div className="rounded border border-border bg-muted/30 p-2.5">
                  <div className="text-[11px] text-muted-foreground">
                    净新增关系类型 ({preview.added_edge_types.length})
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {preview.added_edge_types.length > 0 ? (
                      preview.added_edge_types.map((name) => (
                        <Badge key={name} variant="neutral">
                          + {name}
                        </Badge>
                      ))
                    ) : (
                      <span className="text-muted-foreground">无</span>
                    )}
                  </div>
                </div>
                <div className="rounded border border-border bg-muted/30 p-2.5">
                  <div className="text-[11px] text-muted-foreground">
                    净新增映射规则 ({preview.added_mappings.length})
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1">
                    {preview.added_mappings.length > 0 ? (
                      preview.added_mappings.map((name) => (
                        <Badge key={name} variant="outline">
                          + {name}
                        </Badge>
                      ))
                    ) : (
                      <span className="text-muted-foreground">无</span>
                    )}
                  </div>
                </div>
              </div>

              {/* Synonymous Entity Alignments */}
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <h3 className="font-semibold text-xs">
                    同义实体候选对齐 ({preview.alignments.length})
                  </h3>
                  <Button
                    variant="ghost"
                    className="h-6 px-2 py-0.5 text-[11px]"
                    onClick={() => void onRefreshPreview(resolutionList)}
                  >
                    <RefreshCw className="h-3 w-3" />
                    按当前策略重新预演
                  </Button>
                </div>

                {preview.alignments.length === 0 ? (
                  <div className="rounded border border-border p-3 text-muted-foreground flex items-center gap-2">
                    <CheckCircle2 className="h-4 w-4 text-emerald-600" />
                    未检测到与主图重叠的同义实体，全部子图类型将直接并入主图。
                  </div>
                ) : (
                  <div className="rounded border border-border overflow-hidden">
                    <table className="w-full text-left border-collapse">
                      <thead className="bg-muted/60 text-[11px] text-muted-foreground">
                        <tr>
                          <th className="px-3 py-2 font-medium">子图类型</th>
                          <th className="px-3 py-2 font-medium">主图对齐目标</th>
                          <th className="px-3 py-2 font-medium">对齐得分</th>
                          <th className="px-3 py-2 font-medium">合并策略</th>
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
                                <select
                                  value={chosen}
                                  onChange={(e) =>
                                    updateResolutionStrategy(
                                      al.sub_type,
                                      al.main_type,
                                      e.target.value as SubgraphMergeStrategy
                                    )
                                  }
                                  className="w-full rounded border border-input bg-background px-2 py-1 text-xs"
                                >
                                  {(
                                    Object.keys(
                                      STRATEGY_LABELS
                                    ) as SubgraphMergeStrategy[]
                                  ).map((st) => (
                                    <option key={st} value={st}>
                                      {STRATEGY_LABELS[st]}
                                    </option>
                                  ))}
                                </select>
                                {chosen === "BRIDGE_LINK" && (
                                  <Input
                                    value={
                                      current?.bridge_edge_name ??
                                      "SAME_ENTITY_AS"
                                    }
                                    onChange={(e) =>
                                      updateBridgeEdge(
                                        al.sub_type,
                                        e.target.value
                                      )
                                    }
                                    placeholder="桥接关系名 (默认 SAME_ENTITY_AS)"
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
                  冲突检测清单 ({preview.conflicts.length})
                </h3>
                {preview.conflicts.length === 0 ? (
                  <div className="rounded border border-border p-2.5 text-muted-foreground">
                    零属性类型、聚合规则、关系或映射冲突。
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {preview.conflicts.map((c, idx) => (
                      <div
                        key={`${c.kind}-${c.sub_item}-${idx}`}
                        className="rounded border border-amber-500/40 bg-amber-500/10 p-2.5 flex items-start gap-2"
                      >
                        <AlertTriangle className="h-4 w-4 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                        <div className="space-y-0.5">
                          <div className="flex items-center gap-2">
                            <Badge variant="neutral">
                              {CONFLICT_KIND_LABELS[c.kind] ?? c.kind}
                            </Badge>
                            <span className="font-mono-tech font-medium">
                              {c.sub_item} ↔ {c.main_item}
                            </span>
                          </div>
                          <p className="text-muted-foreground">{c.detail}</p>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {/* Default Strategy & Commit Summary */}
              <div className="grid grid-cols-1 md:grid-cols-2 gap-3 pt-2 border-t border-border">
                <div>
                  <label className="block font-medium mb-1">
                    默认对齐策略 (default_strategy)
                  </label>
                  <select
                    data-testid="merge-default-strategy"
                    value={defaultStrategy}
                    onChange={(e) =>
                      setDefaultStrategy(
                        e.target.value as SubgraphMergeStrategy
                      )
                    }
                    className="h-8 w-full rounded border border-input bg-background px-2 text-xs"
                  >
                    {(
                      Object.keys(STRATEGY_LABELS) as SubgraphMergeStrategy[]
                    ).map((st) => (
                      <option key={st} value={st}>
                        {STRATEGY_LABELS[st]}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="block font-medium mb-1">
                    主图版本发布摘要 (change_summary)
                  </label>
                  <Input
                    data-testid="merge-change-summary"
                    value={changeSummary}
                    onChange={(e) => setChangeSummary(e.target.value)}
                    placeholder="输入本次合并到主图的变更说明"
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
                    强制合并 (force=true，忽略破坏性变更警告提交主图事务)
                  </span>
                </label>
              )}
            </>
          ) : null}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
          <Button variant="outline" onClick={onClose} disabled={merging}>
            取消
          </Button>
          <Button
            data-testid="execute-subgraph-merge-btn"
            onClick={() => void handleMerge()}
            disabled={loading || merging || !preview}
          >
            <GitMerge className="h-3.5 w-3.5" />
            {merging ? "合并发布中..." : "确认合并至企业主图"}
          </Button>
        </div>
      </div>
    </div>
  );
}
