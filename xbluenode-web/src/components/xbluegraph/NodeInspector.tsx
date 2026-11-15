import { useCallback, useEffect, useState } from "react";
import {
  Check,
  FileText,
  Link2,
  MapPin,
  Share2,
  TrendingUp,
  X,
} from "lucide-react";
import {
  xblueApi,
  type BlueNode,
  type EntityLink,
  type NodeDocumentHit,
} from "@/lib/api/xblue";
import { Badge, Button } from "@/components/ui/primitives";
import { MetricChart } from "@/components/xbluegraph/MetricChart";
import { MetricExplainChain } from "@/components/xbluegraph/MetricExplainChain";

export interface NodeInspectorProps {
  node: BlueNode | null;
  onExpandOneHop?: (nodeId: string) => Promise<void>;
}

export function NodeInspector({ node, onExpandOneHop }: NodeInspectorProps) {
  const [selectedMetric, setSelectedMetric] = useState<string | null>(null);
  const [documents, setDocuments] = useState<NodeDocumentHit[]>([]);
  const [candidates, setCandidates] = useState<EntityLink[]>([]);
  const [resolving, setResolving] = useState(false);
  const [actionMsg, setActionMsg] = useState<string | null>(null);

  const loadSideData = useCallback(async (nodeId: string) => {
    const [docsRes, candRes] = await Promise.all([
      xblueApi.getNodeDocuments(nodeId).catch(() => ({ items: [] })),
      xblueApi.listEntityCandidates().catch(() => ({ items: [] })),
    ]);
    setDocuments(Array.isArray(docsRes?.items) ? docsRes.items : []);
    const allCands = Array.isArray(candRes?.items) ? candRes.items : [];
    setCandidates(
      allCands.filter((c) => c.node_id === nodeId && !c.rejected)
    );
  }, []);

  useEffect(() => {
    setActionMsg(null);
    if (!node) {
      setSelectedMetric(null);
      setDocuments([]);
      setCandidates([]);
      return;
    }
    const metricKeys = Object.keys(node.metrics ?? {});
    setSelectedMetric((prev) => {
      if (prev && metricKeys.includes(prev)) {
        return prev;
      }
      if (metricKeys.includes("inventory")) {
        return "inventory";
      }
      return metricKeys[0] ?? null;
    });
    void loadSideData(node.id);
  }, [node, loadSideData]);

  const handleResolveEntities = async () => {
    if (!node) return;
    setResolving(true);
    setActionMsg(null);
    try {
      const res = await xblueApi.resolveEntities({ node_id: node.id });
      setActionMsg(
        `对齐完成：解析 ${res.resolved} 项，自动确认 ${res.auto_confirmed} 项，待确认 ${res.pending_candidates} 项`
      );
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setResolving(false);
    }
  };

  const handleConfirm = async (cand: EntityLink) => {
    if (!node) return;
    try {
      await xblueApi.confirmEntityLink(cand.entity_id, cand.node_id);
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleReject = async (cand: EntityLink) => {
    if (!node) return;
    try {
      await xblueApi.rejectEntityLink(cand.entity_id, cand.node_id);
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    }
  };

  if (!node) {
    return (
      <div
        className="py-8 text-center text-xs text-muted-foreground"
        data-testid="node-inspector-empty"
      >
        点击左侧拓扑画布或节点列表中的任意节点，查看属性、最新指标趋势、关联知识文档与实体链接。
      </div>
    );
  }

  const metricEntries = Object.entries(node.metrics ?? {});
  const propEntries = Object.entries(node.properties ?? {}).filter(
    ([k]) => !k.startsWith("__")
  );

  return (
    <div className="space-y-4 text-xs" data-testid="node-inspector">
      {/* Node Header */}
      <div className="space-y-2 border-b border-border pb-3">
        <div className="flex items-start justify-between gap-2">
          <div>
            <div className="text-sm font-semibold flex items-center gap-2">
              <span data-testid="inspector-node-title">{node.title}</span>
              <Badge variant="xblue">{node.type_name}</Badge>
            </div>
            <div className="font-mono-tech text-[11px] text-muted-foreground mt-0.5">
              node_key:{" "}
              <span
                className="text-foreground font-medium"
                data-testid="inspector-node-key"
              >
                {node.node_key}
              </span>
              {node.row_version !== undefined && (
                <span className="ml-2">· v{node.row_version}</span>
              )}
            </div>
          </div>

          {onExpandOneHop && (
            <Button
              type="button"
              variant="outline"
              className="px-2.5 py-1 text-[11px]"
              onClick={() => void onExpandOneHop(node.id)}
              data-testid="inspector-expand-btn"
            >
              <Share2 className="h-3 w-3" />
              展开 1 跳
            </Button>
          )}
        </div>

        {node.geo && (
          <div
            className="inline-flex items-center gap-1.5 rounded bg-muted px-2 py-1 font-mono-tech text-[11px]"
            data-testid="inspector-node-geo"
          >
            <MapPin className="h-3 w-3 text-xblue" />
            经纬度: {node.geo.lng.toFixed(4)}, {node.geo.lat.toFixed(4)}
          </div>
        )}
      </div>

      {/* Static Properties */}
      {propEntries.length > 0 && (
        <div className="space-y-1.5">
          <div className="font-semibold text-muted-foreground">静态属性</div>
          <div className="grid grid-cols-1 gap-1 rounded border border-border p-2 bg-muted/20">
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

      {/* Latest Metrics & Click-to-Plot Trend */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <span className="font-semibold flex items-center gap-1.5">
            <TrendingUp className="h-3.5 w-3.5 text-xblue" />
            最新时序指标快照
          </span>
          <span className="text-[11px] text-muted-foreground">
            点击指标查看趋势
          </span>
        </div>

        {metricEntries.length === 0 ? (
          <div className="rounded border border-dashed border-border py-3 text-center text-muted-foreground">
            暂无实时指标数值
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-1.5" data-testid="inspector-metrics-list">
            {metricEntries.map(([mKey, mVal]) => {
              const isSelected = selectedMetric === mKey;
              return (
                <button
                  key={mKey}
                  type="button"
                  onClick={() => setSelectedMetric(mKey)}
                  data-testid={`metric-item-${mKey}`}
                  className={`rounded border px-2.5 py-2 text-left transition-colors cursor-pointer ${
                    isSelected
                      ? "border-xblue bg-xblue-soft text-xblue"
                      : "border-border bg-background hover:bg-muted"
                  }`}
                >
                  <div className="font-mono-tech text-[11px] opacity-80">
                    {mKey}
                  </div>
                  <div className="font-mono-tech text-sm font-semibold mt-0.5">
                    {typeof mVal === "number"
                      ? Number.isInteger(mVal)
                        ? mVal
                        : mVal.toFixed(2)
                      : String(mVal)}
                  </div>
                </button>
              );
            })}
          </div>
        )}

        {selectedMetric && (
          <>
            <MetricChart
              nodeId={node.id}
              nodeTitle={node.title}
              metric={selectedMetric}
            />
            <MetricExplainChain nodeId={node.id} metric={selectedMetric} />
          </>
        )}
      </div>

      {/* Associated Knowledge Documents */}
      <div className="space-y-1.5 border-t border-border pt-3">
        <div className="font-semibold flex items-center gap-1.5">
          <FileText className="h-3.5 w-3.5 text-xblue" />
          关联超图知识文档 ({documents.length})
        </div>

        {documents.length === 0 ? (
          <div className="text-[11px] text-muted-foreground">
            暂无关联提及文档。
          </div>
        ) : (
          <div className="space-y-1.5" data-testid="inspector-documents-list">
            {documents.map((doc) => (
              <div
                key={`${doc.doc_id}-${doc.entity_id}`}
                className="rounded border border-border p-2 space-y-1 bg-background"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">{doc.title}</span>
                  <Badge variant="neutral">{doc.entity_name}</Badge>
                </div>
                {doc.snippets.slice(0, 2).map((snip, idx) => (
                  <p
                    key={idx}
                    className="text-[11px] text-muted-foreground leading-relaxed"
                  >
                    {snip}
                  </p>
                ))}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Entity Link Candidates Confirmation */}
      <div className="space-y-2 border-t border-border pt-3">
        <div className="flex items-center justify-between">
          <span className="font-semibold flex items-center gap-1.5">
            <Link2 className="h-3.5 w-3.5 text-xblue" />
            实体链接候选 ({candidates.length})
          </span>
          <Button
            type="button"
            variant="outline"
            className="px-2 py-1 text-[11px]"
            disabled={resolving}
            onClick={() => void handleResolveEntities()}
            data-testid="resolve-entities-btn"
          >
            {resolving ? "对齐中..." : "触发实体对齐"}
          </Button>
        </div>

        {actionMsg && (
          <div className="rounded bg-muted px-2 py-1 text-[11px]">
            {actionMsg}
          </div>
        )}

        {candidates.length === 0 ? (
          <div className="text-[11px] text-muted-foreground">
            暂无待人工确认的实体链接候选。
          </div>
        ) : (
          <div className="space-y-1.5" data-testid="entity-candidates-list">
            {candidates.map((cand) => (
              <div
                key={`${cand.entity_id}-${cand.node_id}`}
                className="rounded border border-border p-2 flex items-center justify-between gap-2 bg-background"
              >
                <div>
                  <div className="font-medium flex items-center gap-1.5">
                    <span>{cand.entity_name}</span>
                    <Badge variant="outline">{cand.match_method}</Badge>
                  </div>
                  <div className="font-mono-tech text-[11px] text-muted-foreground">
                    置信度 {(cand.confidence * 100).toFixed(1)}%
                    {cand.confirmed ? " · 已确认" : ""}
                  </div>
                </div>
                {!cand.confirmed && (
                  <div className="flex items-center gap-1">
                    <Button
                      type="button"
                      variant="outline"
                      className="px-2 py-1 text-[11px]"
                      onClick={() => void handleConfirm(cand)}
                      data-testid={`confirm-entity-${cand.entity_id}`}
                    >
                      <Check className="h-3 w-3 text-xblue" />
                      确认
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      className="px-2 py-1 text-[11px]"
                      onClick={() => void handleReject(cand)}
                      data-testid={`reject-entity-${cand.entity_id}`}
                    >
                      <X className="h-3 w-3" />
                      拒绝
                    </Button>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
