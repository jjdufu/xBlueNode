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
} from "lucide-react";
import { useI18n } from "@/i18n";
import { axflowApi, type AlertRecord } from "@/lib/api/axflow";
import { axsimApi } from "@/lib/api/axsim";
import {
  xblueApi,
  type BlueNode,
  type NodeDocumentHit,
} from "@/lib/api/xblue";
import { xblueviewApi, type WidgetRecord } from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { Badge, Button, Drawer, StatusBadge } from "@/components/ui/primitives";
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
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canSimRun = can("SIM_RUN");
  const canAlertAck = can("ALERT_ACK");

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
            ctx.metricName &&
            ctx.metricValue !== null &&
            ctx.metricValue !== undefined
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

  const handleAckAlert = async (alertId: string) => {
    if (!canAlertAck) return;
    try {
      await axflowApi.ackAlert(alertId, {
        comment: tx("节点详情确认", "Acknowledged in node drawer"),
      });
      setAlerts((prev) =>
        prev.map((a) => (a.id === alertId ? { ...a, status: "ACK" } : a))
      );
      onActionExecuted?.();
    } catch (err) {
      setBusyMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateSandboxFromNode = async () => {
    if (!node || !canSimRun) return;
    setBusyMsg(tx("正在克隆推演沙盘…", "Cloning simulation sandbox…"));
    try {
      const sb = await axsimApi.createSandbox({
        name: tx(`${node.title} 推演沙盘`, `${node.title} Sandbox`),
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
    setBusyMsg(
      tx(`正在执行「${w.title}」…`, `Running "${w.title}"…`)
    );
    try {
      await xblueviewApi.invokeWidgetAction(w.id, {
        inputs: {},
        target_node_id: node.id,
        idempotency_key: `drawer-${w.id}-${node.id}-${Date.now()}`,
      });
      setBusyMsg(
        tx(`已执行「${w.title}」`, `Completed "${w.title}"`)
      );
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
    <Drawer
      open={Boolean(selected)}
      onClose={onClose}
      testId="node-drawer"
      closeAriaLabel={tx("关闭节点抽屉", "Close Node Drawer")}
      widthClass="max-w-md"
      title={
        selected ? (
          <div className="flex items-center gap-2 min-w-0">
            <span className="truncate">
              {node?.title ??
                selected.title ??
                selected.nodeKey ??
                tx("节点详情", "Node Detail")}
            </span>
            <Badge variant="xblue">
              {node?.type_name ?? selected.typeName ?? "Node"}
            </Badge>
          </div>
        ) : (
          tx("节点详情", "Node Detail")
        )
      }
    >
      {selected && (
        <div className="space-y-4">
          <div className="text-[11px] font-mono-tech text-muted-foreground">
            key: {node?.node_key ?? selected.nodeKey ?? selected.nodeId}
          </div>

          {!readOnly && node && (
            <div className="flex flex-wrap gap-2 pb-2 border-b border-border">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  onClose();
                  void navigate(`/graph?node_id=${encodeURIComponent(node.id)}`);
                }}
              >
                <Share2 className="h-3.5 w-3.5 text-xblue" />
                {tx("查看图谱", "Open Graph")}
              </Button>
              {canSimRun && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => void handleCreateSandboxFromNode()}
                >
                  <GitBranch className="h-3.5 w-3.5 text-xblue" />
                  {tx("新建沙盘", "New Sandbox")}
                </Button>
              )}
            </div>
          )}

          {loading && (
            <div className="text-muted-foreground">
              {tx("加载中…", "Loading…")}
            </div>
          )}

          {busyMsg && (
            <div className="rounded border border-xblue/30 bg-xblue-soft/50 px-2.5 py-1.5 text-[11px]">
              {busyMsg}
            </div>
          )}

          {propEntries.length > 0 && (
            <div className="space-y-1.5">
              <div className="font-semibold text-muted-foreground">
                {tx("节点属性", "Properties")}
              </div>
              <div className="rounded border border-border p-2 space-y-1 bg-muted/20">
                {propEntries.map(([k, v]) => (
                  <div
                    key={k}
                    className="flex items-center justify-between gap-2"
                  >
                    <span className="font-mono-tech text-muted-foreground">
                      {k}
                    </span>
                    <span className="font-mono-tech font-medium truncate">
                      {typeof v === "number"
                        ? fmtNum(v, 4)
                        : typeof v === "object" && v !== null
                          ? JSON.stringify(v)
                          : String(v ?? "")}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}

          <div className="space-y-2">
            <div className="font-semibold flex items-center gap-1.5">
              <TrendingUp className="h-3.5 w-3.5 text-xblue" />
              {tx("实时指标与曲线", "Metrics & Trend")}
            </div>
            {metricEntries.length === 0 ? (
              <div className="text-muted-foreground">
                {tx("暂无时序指标", "No metrics")}
              </div>
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
                    <div className="font-mono-tech text-[10px] opacity-80 truncate">
                      {k}
                    </div>
                    <div className="font-mono-tech text-sm font-semibold tabular-nums">
                      {typeof v === "number" ? fmtNum(v, 4) : String(v ?? "-")}
                    </div>
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

          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="font-semibold flex items-center gap-1.5">
              <AlertTriangle className="h-3.5 w-3.5 text-xblue" />
              {tx("关联告警", "Alerts")} ({alerts.length})
            </div>
            {alerts.length === 0 ? (
              <div className="text-muted-foreground">
                {tx("暂无关联告警", "No alerts")}
              </div>
            ) : (
              <div className="space-y-1.5">
                {alerts.map((a) => (
                  <div
                    key={a.id}
                    className="rounded border border-border p-2 flex items-center justify-between gap-2"
                  >
                    <div className="min-w-0 space-y-0.5">
                      <div className="font-medium truncate">{a.title}</div>
                      <div className="flex items-center gap-1.5">
                        <StatusBadge status={a.severity} />
                        <StatusBadge status={a.status} />
                      </div>
                    </div>
                    {!readOnly && canAlertAck && a.status === "OPEN" && (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => void handleAckAlert(a.id)}
                      >
                        <CheckCircle2 className="h-3 w-3 text-xblue" />
                        {tx("确认", "Ack")}
                      </Button>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>

          {!readOnly && actionWidgets.length > 0 && (
            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="font-semibold">
                {tx("绑定动作", "Bound Actions")}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {actionWidgets.map((aw) => (
                  <Button
                    key={aw.id}
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void handleInvokeBoundAction(aw)}
                  >
                    <Play className="h-3 w-3 text-xblue" />
                    {aw.title || tx("执行", "Run")}
                  </Button>
                ))}
              </div>
            </div>
          )}

          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="font-semibold flex items-center gap-1.5">
              <FileText className="h-3.5 w-3.5 text-xblue" />
              {tx("关联文档", "Documents")} ({docs.length})
            </div>
            {docs.length === 0 ? (
              <div className="text-muted-foreground">
                {tx("暂无关联文档", "No documents")}
              </div>
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
      )}
    </Drawer>
  );
}
