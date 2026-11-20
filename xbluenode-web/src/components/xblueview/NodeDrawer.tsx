import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import {
  AlertTriangle,
  CheckCircle2,
  FileText,
  GitBranch,
  Play,
  Share2,
  TrendingUp,
  X,
} from "lucide-react";
import { axflowApi, type AlertRecord } from "@/lib/api/axflow";
import { axsimApi } from "@/lib/api/axsim";
import {
  xblueApi,
  type BlueNode,
  type NodeDocumentHit,
} from "@/lib/api/xblue";
import { xblueviewApi, type WidgetRecord } from "@/lib/api/xblueview";
import { Badge, Button } from "@/components/ui/primitives";
import { MetricChart } from "@/components/xbluegraph/MetricChart";
import type { SelectedNodeContext } from "./utils";

export interface NodeDrawerProps {
  selected: SelectedNodeContext | null;
  actionWidgets?: WidgetRecord[];
  readOnly?: boolean;
  onClose: () => void;
  onActionExecuted?: () => void;
}

export function NodeDrawer({
  selected,
  actionWidgets = [],
  readOnly = false,
  onClose,
  onActionExecuted,
}: NodeDrawerProps) {
  const navigate = useNavigate();
  const [node, setNode] = useState<BlueNode | null>(null);
  const [selectedMetric, setSelectedMetric] = useState<string | null>(null);
  const [docs, setDocs] = useState<NodeDocumentHit[]>([]);
  const [alerts, setAlerts] = useState<AlertRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyMsg, setBusyMsg] = useState<string | null>(null);

  const loadNodeDetails = useCallback(async (ctx: SelectedNodeContext) => {
    setLoading(true);
    setBusyMsg(null);
    try {
      let loaded: BlueNode | null = null;
      const isUuid =
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
          ctx.nodeId
        );

      if (isUuid) {
        loaded = await xblueApi.getNode(ctx.nodeId).catch(() => null);
      }
      if (!loaded && ctx.typeName && ctx.nodeKey) {
        const domainObj = await xblueApi
          .getDomainObject(ctx.typeName, ctx.nodeKey)
          .catch(() => null);
        if (domainObj && domainObj.id) {
          loaded = {
            id: domainObj.id,
            type_name: domainObj.type_name,
            node_key: domainObj.node_key,
            title: domainObj.title,
            properties: domainObj.attributes,
            metrics: domainObj.metrics,
            geo: domainObj.geo ?? null,
          };
        }
      }
      if (!loaded) {
        const searchRes = await xblueApi
          .searchNodes({
            q: ctx.nodeKey ?? ctx.title ?? ctx.nodeId,
            type_name: ctx.typeName,
            k: 1,
          })
          .catch(() => ({ items: [] }));
        loaded = searchRes.items[0] ?? null;
      }

      if (!loaded) {
        loaded = {
          id: ctx.nodeId,
          type_name: ctx.typeName ?? "Node",
          node_key: ctx.nodeKey ?? ctx.nodeId,
          title: ctx.title ?? ctx.nodeKey ?? ctx.nodeId,
          properties: {},
          metrics:
            ctx.metricName && ctx.metricValue !== null && ctx.metricValue !== undefined
              ? { [ctx.metricName]: ctx.metricValue }
              : {},
        };
      }

      setNode(loaded);
      const mKeys = Object.keys(loaded.metrics ?? {});
      setSelectedMetric(
        ctx.metricName && mKeys.includes(ctx.metricName)
          ? ctx.metricName
          : mKeys[0] ?? null
      );

      if (loaded.id) {
        const [docRes, alertRes] = await Promise.all([
          xblueApi.getNodeDocuments(loaded.id).catch(() => ({ items: [] })),
          axflowApi
            .listAlerts({ node_id: loaded.id, limit: 10 })
            .catch(() => ({ items: [], next_cursor: null })),
        ]);
        setDocs(Array.isArray(docRes?.items) ? docRes.items : []);
        setAlerts(Array.isArray(alertRes?.items) ? alertRes.items : []);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!selected) {
      setNode(null);
      setDocs([]);
      setAlerts([]);
      return;
    }
    void loadNodeDetails(selected);
  }, [loadNodeDetails, selected]);

  if (!selected) return null;

  const handleAckAlert = async (alertId: string) => {
    try {
      await axflowApi.ackAlert(alertId, { comment: "节点详情抽屉确认" });
      setAlerts((prev) =>
        prev.map((a) => (a.id === alertId ? { ...a, status: "ACK" } : a))
      );
      onActionExecuted?.();
    } catch (err) {
      setBusyMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateSandboxFromNode = async () => {
    if (!node) return;
    setBusyMsg("正在基于当前节点克隆推演沙盘...");
    try {
      const sb = await axsimApi.createSandbox({
        name: `${node.title} 推演沙盘`,
        scope: {
          root_node_ids: [node.id],
          depth: 2,
          direction: "BOTH",
        },
      });
      onClose();
      void navigate(`/sim?sandbox_id=${encodeURIComponent(sb.id)}`);
    } catch (err) {
      setBusyMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleInvokeBoundAction = async (w: WidgetRecord) => {
    if (!node || readOnly) return;
    setBusyMsg(`正在执行动作「${w.title}」...`);
    try {
      await xblueviewApi.invokeWidgetAction(w.id, {
        inputs: {},
        target_node_id: node.id,
        idempotency_key: `drawer-${w.id}-${node.id}-${Date.now()}`,
      });
      setBusyMsg(`动作「${w.title}」已执行`);
      onActionExecuted?.();
    } catch (err) {
      setBusyMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const propEntries = Object.entries(node?.properties ?? {}).filter(
    ([k]) => !k.startsWith("__")
  );
  const metricEntries = Object.entries(node?.metrics ?? {});

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      data-testid="node-drawer-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md h-full bg-background border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="node-drawer"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-4 py-3 border-b border-border flex items-start justify-between gap-2">
          <div>
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-semibold">
                {node?.title ?? selected.title ?? selected.nodeKey ?? "节点详情"}
              </h3>
              <Badge variant="xblue">
                {node?.type_name ?? selected.typeName ?? "Node"}
              </Badge>
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground mt-0.5">
              key: {node?.node_key ?? selected.nodeKey ?? selected.nodeId}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            aria-label="关闭节点抽屉"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Navigation / Twin Actions */}
        {!readOnly && node && (
          <div className="px-4 py-2 border-b border-border flex flex-wrap gap-2 bg-muted/30">
            <Button
              type="button"
              variant="outline"
              className="px-2.5 py-1 text-xs"
              onClick={() => {
                onClose();
                void navigate(`/graph?node_id=${encodeURIComponent(node.id)}`);
              }}
            >
              <Share2 className="h-3.5 w-3.5 text-xblue" />
              在图浏览器中打开
            </Button>
            <Button
              type="button"
              variant="outline"
              className="px-2.5 py-1 text-xs"
              onClick={() => void handleCreateSandboxFromNode()}
            >
              <GitBranch className="h-3.5 w-3.5 text-xblue" />
              以此节点建沙盘
            </Button>
          </div>
        )}

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
          {loading && (
            <div className="text-muted-foreground">正在加载节点孪生上下文...</div>
          )}

          {busyMsg && (
            <div className="rounded border border-xblue/30 bg-xblue-soft/50 px-2.5 py-1.5 text-[11px]">
              {busyMsg}
            </div>
          )}

          {/* Properties */}
          {propEntries.length > 0 && (
            <div className="space-y-1.5">
              <div className="font-semibold text-muted-foreground">节点属性</div>
              <div className="rounded border border-border p-2 space-y-1 bg-muted/20">
                {propEntries.map(([k, v]) => (
                  <div key={k} className="flex items-center justify-between gap-2">
                    <span className="font-mono-tech text-muted-foreground">{k}</span>
                    <span className="font-mono-tech font-medium truncate">
                      {typeof v === "object" && v !== null
                        ? JSON.stringify(v)
                        : String(v ?? "")}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Metrics & Trend */}
          <div className="space-y-2">
            <div className="font-semibold flex items-center gap-1.5">
              <TrendingUp className="h-3.5 w-3.5 text-xblue" />
              实时指标与曲线
            </div>
            {metricEntries.length === 0 ? (
              <div className="text-muted-foreground">暂无时序指标</div>
            ) : (
              <div className="grid grid-cols-2 gap-1.5">
                {metricEntries.map(([k, v]) => (
                  <button
                    key={k}
                    type="button"
                    onClick={() => setSelectedMetric(k)}
                    className={`rounded border px-2 py-1.5 text-left cursor-pointer ${
                      selectedMetric === k
                        ? "border-xblue bg-xblue-soft text-xblue"
                        : "border-border hover:bg-muted"
                    }`}
                  >
                    <div className="font-mono-tech text-[10px] opacity-80">{k}</div>
                    <div className="font-mono-tech text-sm font-semibold">{v}</div>
                  </button>
                ))}
              </div>
            )}
            {node?.id && selectedMetric && (
              <MetricChart
                nodeId={node.id}
                nodeTitle={node.title}
                metric={selectedMetric}
              />
            )}
          </div>

          {/* Alerts */}
          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="font-semibold flex items-center gap-1.5">
              <AlertTriangle className="h-3.5 w-3.5 text-xblue" />
              相关告警 ({alerts.length})
            </div>
            {alerts.length === 0 ? (
              <div className="text-muted-foreground">暂无关联告警</div>
            ) : (
              <div className="space-y-1.5">
                {alerts.map((a) => (
                  <div
                    key={a.id}
                    className="rounded border border-border p-2 flex items-center justify-between gap-2"
                  >
                    <div className="min-w-0">
                      <div className="font-medium truncate">{a.title}</div>
                      <div className="text-[10px] font-mono-tech text-muted-foreground">
                        {a.severity} · {a.status}
                      </div>
                    </div>
                    {!readOnly && a.status === "OPEN" && (
                      <Button
                        type="button"
                        variant="outline"
                        className="px-2 py-1 text-[11px]"
                        onClick={() => void handleAckAlert(a.id)}
                      >
                        <CheckCircle2 className="h-3 w-3 text-xblue" />
                        确认
                      </Button>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Bound Action Buttons */}
          {!readOnly && actionWidgets.length > 0 && (
            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="font-semibold">绑定业务动作</div>
              <div className="flex flex-wrap gap-1.5">
                {actionWidgets.map((aw) => (
                  <Button
                    key={aw.id}
                    type="button"
                    variant="outline"
                    className="px-2.5 py-1 text-xs"
                    onClick={() => void handleInvokeBoundAction(aw)}
                  >
                    <Play className="h-3 w-3 text-xblue" />
                    {aw.title || "执行动作"}
                  </Button>
                ))}
              </div>
            </div>
          )}

          {/* Documents */}
          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="font-semibold flex items-center gap-1.5">
              <FileText className="h-3.5 w-3.5 text-xblue" />
              关联超图文档 ({docs.length})
            </div>
            {docs.length === 0 ? (
              <div className="text-muted-foreground">暂无关联文档</div>
            ) : (
              <div className="space-y-1.5">
                {docs.map((doc) => (
                  <div
                    key={`${doc.doc_id}-${doc.entity_id}`}
                    className="rounded border border-border p-2 space-y-1"
                  >
                    <div className="font-medium">{doc.title}</div>
                    {doc.snippets.slice(0, 2).map((snip, i) => (
                      <p key={i} className="text-[11px] text-muted-foreground">
                        {snip}
                      </p>
                    ))}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
