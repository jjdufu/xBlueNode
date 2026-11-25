import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import {
  BarChart3,
  Bot,
  CopyPlus,
  Database,
  GitBranch,
  Monitor,
  Package,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  axopsApi,
  type TemplateManifest,
  type TemplatePreviewResponse,
} from "@/lib/api/axops";
import { xblueApi } from "@/lib/api/xblue";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Modal,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface TemplateHubModalProps {
  open: boolean;
  initialPackId?: string | null;
  onClose: () => void;
}

export function TemplateHubModal({
  open,
  initialPackId = null,
  onClose,
}: TemplateHubModalProps) {
  const navigate = useNavigate();
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canClone = can("STUDIO_EDIT");

  const [packs, setPacks] = useState<TemplateManifest[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [preview, setPreview] = useState<TemplatePreviewResponse | null>(null);
  const [loadingList, setLoadingList] = useState(false);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [cloning, setCloning] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadList = useCallback(async () => {
    setLoadingList(true);
    setErrorMsg(null);
    try {
      const res = await axopsApi.listTemplates();
      const items = Array.isArray(res?.items) ? res.items : [];
      setPacks(items);
      setSelectedId((prev) => {
        if (initialPackId && items.some((p) => p.id === initialPackId)) {
          return initialPackId;
        }
        if (prev && items.some((p) => p.id === prev)) {
          return prev;
        }
        return items[0]?.id ?? null;
      });
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("加载模板失败", "Failed to load templates")
      );
    } finally {
      setLoadingList(false);
    }
  }, [initialPackId, tx]);

  useEffect(() => {
    if (!open) return;
    if (initialPackId) {
      setSelectedId(initialPackId);
    }
    void loadList();
  }, [open, initialPackId, loadList]);

  useEffect(() => {
    if (!open || !selectedId) {
      setPreview(null);
      return;
    }
    let cancelled = false;
    setLoadingPreview(true);
    setErrorMsg(null);
    void axopsApi
      .getTemplatePreview(selectedId)
      .then((res) => {
        if (!cancelled) {
          setPreview(res);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setPreview(null);
          setErrorMsg(
            err instanceof Error
              ? err.message
              : tx("加载预览失败", "Failed to load preview")
          );
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingPreview(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, selectedId, tx]);

  const handleTrialClone = async () => {
    if (!selectedId || !canClone || cloning) return;
    setCloning(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.cloneSubgraphFromTemplate({
        pack_id: selectedId,
      });
      onClose();
      const targetUrl = res.draft_id
        ? `/studio?subgraph=${encodeURIComponent(res.subgraph_slug)}&draft=${encodeURIComponent(res.draft_id)}`
        : `/studio?subgraph=${encodeURIComponent(res.subgraph_slug)}`;
      void navigate(targetUrl);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("试用克隆失败", "Trial clone failed")
      );
    } finally {
      setCloning(false);
    }
  };

  const activeManifest =
    preview?.manifest ?? packs.find((p) => p.id === selectedId) ?? null;
  const nodeTypes = preview?.topology_summary?.node_types ?? [];
  const edgeTypes = preview?.topology_summary?.edge_types ?? [];
  const sampleMetrics = preview?.sample_metrics ?? [];
  const dashboardPreviews = preview?.dashboard_previews ?? [];
  const agentPreviews = preview?.agents ?? [];
  const sampleRows = preview?.sample_rows ?? [];

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="template-hub-modal"
      maxWidthClass="max-w-4xl"
      title={
        <>
          <Package className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("行业模板中心", "Template Hub")}</span>
        </>
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={cloning}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="button"
            variant="primary"
            data-testid="template-trial-clone-btn"
            disabled={!canClone || !selectedId || cloning}
            onClick={() => void handleTrialClone()}
          >
            <CopyPlus className="h-3.5 w-3.5" />
            {cloning
              ? tx("克隆中…", "Cloning…")
              : tx("试用克隆", "Try Clone")}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        {errorMsg && (
          <div
            role="alert"
            className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300"
          >
            {errorMsg}
          </div>
        )}

        {loadingList && packs.length === 0 ? (
          <Card className="p-8 text-center text-xs text-muted-foreground">
            {tx("加载中…", "Loading…")}
          </Card>
        ) : packs.length === 0 ? (
          <EmptyState title={tx("暂无模板包", "No template packs")} />
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-12 gap-3">
            {/* Left Pack Selector */}
            <div className="md:col-span-4 space-y-2">
              {packs.map((pack) => {
                const active = pack.id === selectedId;
                return (
                  <button
                    key={pack.id}
                    type="button"
                    data-testid={`template-hub-pack-${pack.id}`}
                    onClick={() => setSelectedId(pack.id)}
                    className={cn(
                      "w-full text-left rounded-md border p-3 space-y-1.5 transition-colors cursor-pointer",
                      active
                        ? "border-xblue bg-xblue-soft/45"
                        : "border-border bg-background hover:bg-muted/40"
                    )}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-xs font-semibold text-foreground truncate">
                        {pack.name}
                      </span>
                      <Badge variant={active ? "xblue" : "outline"}>
                        {pack.industry}
                      </Badge>
                    </div>
                    <div className="font-mono-tech text-[11px] text-muted-foreground">
                      {pack.id} · v{pack.version}
                    </div>
                    <p className="text-[11px] text-muted-foreground line-clamp-2 leading-snug">
                      {pack.description}
                    </p>
                  </button>
                );
              })}
            </div>

            {/* Right Rich Topology & Sample Data Preview */}
            <div
              className="md:col-span-8 space-y-3"
              data-testid="template-preview-pane"
            >
              {activeManifest && (
                <Card className="p-3.5 space-y-2">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className="text-sm font-semibold truncate">
                        {activeManifest.name}
                      </span>
                      <Badge variant="outline">v{activeManifest.version}</Badge>
                      <Badge variant="xblue">{activeManifest.industry}</Badge>
                    </div>
                    <Badge variant="neutral">
                      {tx("隔离演练沙盒", "Sandbox Clone")}
                    </Badge>
                  </div>
                  <p className="text-xs text-muted-foreground leading-relaxed">
                    {activeManifest.description}
                  </p>
                </Card>
              )}

              {loadingPreview && !preview ? (
                <Card className="p-8 text-center text-xs text-muted-foreground">
                  {tx("加载中…", "Loading…")}
                </Card>
              ) : preview ? (
                <>
                  {/* Topology Summary */}
                  <Card className="p-3.5 space-y-2.5">
                    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2">
                      <span className="text-xs font-semibold flex items-center gap-1.5">
                        <GitBranch className="h-3.5 w-3.5 text-xblue" />
                        {tx("模型拓扑摘要", "Model Topology")}
                      </span>
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Badge variant="outline">
                          {tx("节点类型", "Types")} {fmtNum(nodeTypes.length, 0)}
                        </Badge>
                        <Badge variant="outline">
                          {tx("关系", "Relations")} {fmtNum(edgeTypes.length, 0)}
                        </Badge>
                        <Badge variant="outline">
                          {tx("聚合规则", "Agg Rules")}{" "}
                          {fmtNum(preview.topology_summary?.agg_rule_count ?? 0, 0)}
                        </Badge>
                        <Badge variant="outline">
                          {tx("派生规则", "Derived Rules")}{" "}
                          {fmtNum(
                            preview.topology_summary?.derived_rule_count ?? 0,
                            0
                          )}
                        </Badge>
                      </div>
                    </div>

                    {nodeTypes.length > 0 && (
                      <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
                        {nodeTypes.map((nt) => (
                          <div
                            key={nt.name}
                            className="rounded border border-border bg-muted/25 p-2 space-y-0.5"
                          >
                            <div className="font-medium text-xs truncate">
                              {nt.display_name || nt.name}
                            </div>
                            <div className="font-mono-tech text-[10px] text-muted-foreground truncate">
                              {nt.name} · {nt.property_count}p · {nt.metric_count}m
                            </div>
                          </div>
                        ))}
                      </div>
                    )}

                    {edgeTypes.length > 0 && (
                      <div className="flex flex-wrap gap-1.5 pt-0.5">
                        {edgeTypes.map((et, idx) => (
                          <Badge
                            key={`${et.name}-${idx}`}
                            variant="neutral"
                            className="text-[10px]"
                          >
                            {et.src_type_name} → {et.name} ({et.cardinality}) →{" "}
                            {et.dst_type_name}
                          </Badge>
                        ))}
                      </div>
                    )}
                  </Card>

                  {/* Metrics, Dashboards & Agents */}
                  {(sampleMetrics.length > 0 ||
                    dashboardPreviews.length > 0 ||
                    agentPreviews.length > 0) && (
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                      <Card className="p-3 space-y-2">
                        <div className="text-xs font-semibold flex items-center gap-1.5">
                          <BarChart3 className="h-3.5 w-3.5 text-xblue" />
                          <span>{tx("认证指标", "Certified Metrics")}</span>
                          <Badge variant="outline">
                            {fmtNum(sampleMetrics.length, 0)}
                          </Badge>
                        </div>
                        {sampleMetrics.length === 0 ? (
                          <div className="text-[11px] text-muted-foreground">
                            —
                          </div>
                        ) : (
                          <div className="flex flex-wrap gap-1.5">
                            {sampleMetrics.map((m) => (
                              <Badge key={m.name} variant="outline">
                                {m.display_name || m.name} ({m.name}
                                {m.unit ? ` · ${m.unit}` : ""})
                              </Badge>
                            ))}
                          </div>
                        )}
                      </Card>

                      <Card className="p-3 space-y-2">
                        <div className="text-xs font-semibold flex items-center gap-1.5">
                          <Monitor className="h-3.5 w-3.5 text-xblue" />
                          <span>{tx("看板与智能体", "Views & Agents")}</span>
                          <Badge variant="outline">
                            {fmtNum(
                              dashboardPreviews.length + agentPreviews.length,
                              0
                            )}
                          </Badge>
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          {dashboardPreviews.map((d) => (
                            <Badge key={d.slug} variant="xblue">
                              {d.name} · {d.widget_count}{" "}
                              {tx("组件", "Widgets")}
                            </Badge>
                          ))}
                          {agentPreviews.map((a) => (
                            <Badge key={a.name} variant="neutral">
                              <Bot className="h-3 w-3 mr-1 text-xblue" />
                              {a.name} · {a.tool_count} {tx("工具", "Tools")}
                            </Badge>
                          ))}
                          {dashboardPreviews.length === 0 &&
                            agentPreviews.length === 0 && (
                              <span className="text-[11px] text-muted-foreground">
                                —
                              </span>
                            )}
                        </div>
                      </Card>
                    </div>
                  )}

                  {/* Sample Rows Preview Table */}
                  {sampleRows.length > 0 && (
                    <Card className="p-3 space-y-2.5">
                      <div className="text-xs font-semibold flex items-center gap-1.5">
                        <Database className="h-3.5 w-3.5 text-xblue" />
                        <span>{tx("样例数据预览", "Sample Data")}</span>
                        <Badge variant="outline">
                          {fmtNum(sampleRows.length, 0)}
                        </Badge>
                      </div>

                      <div className="space-y-2.5 max-h-56 overflow-y-auto pr-0.5">
                        {sampleRows.map((sr) => (
                          <div
                            key={sr.dataset_name}
                            className="rounded border border-border overflow-hidden"
                          >
                            <div className="flex items-center justify-between bg-muted/30 px-2.5 py-1 text-[11px]">
                              <span className="font-mono-tech font-semibold">
                                {sr.dataset_name}
                              </span>
                              <span className="text-muted-foreground font-mono-tech">
                                {fmtNum(sr.total_rows, 0)} {tx("行", "rows")}
                              </span>
                            </div>
                            <div className="overflow-x-auto">
                              <table className="w-full text-left border-collapse text-[11px]">
                                <thead>
                                  <tr className="border-b border-border bg-muted/15 text-muted-foreground font-mono-tech">
                                    {(sr.headers ?? []).map((h) => (
                                      <th
                                        key={h}
                                        className="py-1 px-2 whitespace-nowrap font-medium"
                                      >
                                        {h}
                                      </th>
                                    ))}
                                  </tr>
                                </thead>
                                <tbody className="divide-y divide-border/60 font-mono-tech">
                                  {(sr.rows ?? []).slice(0, 4).map((row, rIdx) => (
                                    <tr key={rIdx}>
                                      {row.map((cell, cIdx) => (
                                        <td
                                          key={cIdx}
                                          className="py-1 px-2 whitespace-nowrap text-foreground/90"
                                        >
                                          {cell}
                                        </td>
                                      ))}
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </div>
                          </div>
                        ))}
                      </div>
                    </Card>
                  )}
                </>
              ) : null}
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}
