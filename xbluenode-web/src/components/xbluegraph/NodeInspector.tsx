import { useCallback, useEffect, useState } from "react";
import {
  Check,
  ChevronRight,
  FileText,
  Link2,
  MapPin,
  Share2,
  TrendingUp,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import {
  xblueApi,
  type BlueNode,
  type EntityLink,
  type NodeDocumentHit,
} from "@/lib/api/xblue";
import { Badge, Button, EmptyState } from "@/components/ui/primitives";
import { translateNodeTypeLabel } from "@/components/graph/SigmaCanvas";

export interface NodeInspectorProps {
  node: BlueNode | null;
  selectedMetric: string | null;
  onSelectMetric: (metric: string) => void;
  onExpandOneHop?: (nodeId: string) => Promise<void>;
}

export function NodeInspector({
  node,
  selectedMetric,
  onSelectMetric,
  onExpandOneHop,
}: NodeInspectorProps) {
  const { isZh, tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

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
      setDocuments([]);
      setCandidates([]);
      return;
    }
    void loadSideData(node.id);
  }, [node, loadSideData]);

  const handleResolveEntities = async () => {
    if (!node || !canEdit) return;
    setResolving(true);
    setActionMsg(null);
    try {
      const res = await xblueApi.resolveEntities({ node_id: node.id });
      setActionMsg(
        tx(
          `对齐完成：解析 ${res.resolved} · 自动确认 ${res.auto_confirmed} · 待确认 ${res.pending_candidates}`,
          `Resolved ${res.resolved} · Auto-confirmed ${res.auto_confirmed} · Pending ${res.pending_candidates}`
        )
      );
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setResolving(false);
    }
  };

  const handleConfirm = async (cand: EntityLink) => {
    if (!node || !canEdit) return;
    try {
      await xblueApi.confirmEntityLink(cand.entity_id, cand.node_id);
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleReject = async (cand: EntityLink) => {
    if (!node || !canEdit) return;
    try {
      await xblueApi.rejectEntityLink(cand.entity_id, cand.node_id);
      await loadSideData(node.id);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : String(err));
    }
  };

  if (!node) {
    return (
      <div data-testid="node-inspector-empty">
        <EmptyState title={tx("未选中业务节点", "No node selected")} />
      </div>
    );
  }

  const metricEntries = Object.entries(node.metrics ?? {});
  const propEntries = Object.entries(node.properties ?? {}).filter(
    ([k]) => !k.startsWith("__")
  );

  return (
    <div className="space-y-3.5 text-xs" data-testid="node-inspector">
      {/* Node Header */}
      <div className="space-y-2 border-b border-border pb-2.5">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="text-sm font-semibold flex flex-wrap items-center gap-1.5">
              <span data-testid="inspector-node-title">{node.title}</span>
              <Badge variant="xblue">
                {translateNodeTypeLabel(node.type_name, isZh)}
              </Badge>
            </div>
            <div className="font-mono-tech text-[11px] text-muted-foreground mt-0.5">
              {tx("业务主键", "Key")}:{" "}
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
              size="sm"
              onClick={() => void onExpandOneHop(node.id)}
              data-testid="inspector-expand-btn"
            >
              <Share2 className="h-3 w-3 text-xblue" />
              {tx("展开 1 跳", "Expand 1-Hop")}
            </Button>
          )}
        </div>

        {node.geo && (
          <div
            className="inline-flex items-center gap-1.5 rounded bg-muted/60 px-2 py-1 font-mono-tech text-[11px]"
            data-testid="inspector-node-geo"
          >
            <MapPin className="h-3 w-3 text-xblue" />
            {tx("坐标", "Geo")}: {node.geo.lng.toFixed(4)},{" "}
            {node.geo.lat.toFixed(4)}
          </div>
        )}
      </div>

      {/* Borderless Static Properties */}
      {propEntries.length > 0 && (
        <div className="space-y-1">
          <div className="text-[11px] font-semibold text-muted-foreground">
            {tx("静态属性", "Properties")}
          </div>
          <div className="divide-y divide-border/60 rounded-md bg-muted/30 px-2.5 py-1">
            {propEntries.map(([k, v]) => (
              <div
                key={k}
                className="flex items-center justify-between gap-2 py-1"
              >
                <span className="font-mono-tech text-muted-foreground">{k}</span>
                <span className="font-mono-tech font-medium text-foreground truncate">
                  {typeof v === "object" && v !== null
                    ? JSON.stringify(v)
                    : String(v ?? "")}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Compact Key-Value Metric Snapshot Rows (Opens Wide Bottom Chart + Explain Panel on click) */}
      <div className="space-y-1.5">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-semibold text-muted-foreground flex items-center gap-1">
            <TrendingUp className="h-3.5 w-3.5 text-xblue" />
            {tx("时序指标快照", "Metric Snapshot")}
          </span>
          <span className="text-[10px] text-muted-foreground">
            {tx("选择指标查看曲线与溯源", "Select to view trend & lineage")}
          </span>
        </div>

        {metricEntries.length === 0 ? (
          <div className="rounded-md bg-muted/30 py-3 text-center text-muted-foreground">
            {tx("暂无实时指标", "No active metrics")}
          </div>
        ) : (
          <div
            className="divide-y divide-border/60 rounded-md bg-muted/25 overflow-hidden"
            data-testid="inspector-metrics-list"
          >
            {metricEntries.map(([mKey, mVal]) => {
              const isSelected = selectedMetric === mKey;
              return (
                <button
                  key={mKey}
                  type="button"
                  onClick={() => onSelectMetric(mKey)}
                  data-testid={`metric-item-${mKey}`}
                  className={`w-full flex items-center justify-between gap-2 px-2.5 py-2 text-left transition-colors cursor-pointer ${
                    isSelected
                      ? "bg-xblue-soft text-xblue font-semibold"
                      : "hover:bg-muted/70 text-foreground"
                  }`}
                >
                  <span className="font-mono-tech text-xs truncate">{mKey}</span>
                  <span className="inline-flex items-center gap-1 font-mono-tech tabular-nums text-xs font-semibold shrink-0">
                    <span>
                      {typeof mVal === "number" ? fmtNum(mVal, 2) : String(mVal)}
                    </span>
                    <ChevronRight className="h-3.5 w-3.5 opacity-60" />
                  </span>
                </button>
              );
            })}
          </div>
        )}
      </div>

      {/* Associated Knowledge Documents */}
      <div className="space-y-1.5 border-t border-border pt-2.5">
        <div className="text-[11px] font-semibold text-muted-foreground flex items-center gap-1">
          <FileText className="h-3.5 w-3.5 text-xblue" />
          {tx("关联知识文档", "Linked Documents")} ({documents.length})
        </div>

        {documents.length === 0 ? (
          <div className="text-[11px] text-muted-foreground">
            {tx("暂无关联文档", "No linked documents")}
          </div>
        ) : (
          <div className="space-y-1.5" data-testid="inspector-documents-list">
            {documents.map((doc) => (
              <div
                key={`${doc.doc_id}-${doc.entity_id}`}
                className="rounded-md bg-muted/30 p-2 space-y-1"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium truncate">{doc.title}</span>
                  <Badge variant="neutral">{doc.entity_name}</Badge>
                </div>
                {doc.snippets.slice(0, 2).map((snip, idx) => (
                  <p
                    key={idx}
                    className="text-[11px] text-muted-foreground leading-relaxed line-clamp-2"
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
      <div className="space-y-2 border-t border-border pt-2.5">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-semibold text-muted-foreground flex items-center gap-1">
            <Link2 className="h-3.5 w-3.5 text-xblue" />
            {tx("实体链接候选", "Entity Candidates")} ({candidates.length})
          </span>
          {canEdit && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={resolving}
              onClick={() => void handleResolveEntities()}
              className="text-xblue"
              data-testid="resolve-entities-btn"
            >
              {resolving
                ? tx("对齐中…", "Resolving…")
                : tx("实体对齐", "Align")}
            </Button>
          )}
        </div>

        {actionMsg && (
          <div className="rounded bg-muted px-2 py-1 text-[11px]">
            {actionMsg}
          </div>
        )}

        {candidates.length === 0 ? (
          <div className="text-[11px] text-muted-foreground">
            {tx("暂无待确认候选", "No pending candidates")}
          </div>
        ) : (
          <div className="space-y-1.5" data-testid="entity-candidates-list">
            {candidates.map((cand) => (
              <div
                key={`${cand.entity_id}-${cand.node_id}`}
                className="rounded-md bg-muted/30 p-2 flex items-center justify-between gap-2"
              >
                <div className="min-w-0">
                  <div className="font-medium flex items-center gap-1.5">
                    <span className="truncate">{cand.entity_name}</span>
                    <Badge variant="outline">{cand.match_method}</Badge>
                  </div>
                  <div className="font-mono-tech text-[11px] text-muted-foreground">
                    {tx("置信度", "Confidence")}{" "}
                    {(cand.confidence * 100).toFixed(1)}%
                    {cand.confirmed
                      ? tx(" · 已确认", " · Confirmed")
                      : ""}
                  </div>
                </div>
                {!cand.confirmed && canEdit && (
                  <div className="flex items-center gap-1 shrink-0">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() => void handleConfirm(cand)}
                      data-testid={`confirm-entity-${cand.entity_id}`}
                    >
                      <Check className="h-3 w-3 text-xblue" />
                      {tx("确认", "Confirm")}
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => void handleReject(cand)}
                      className="hover:text-red-600"
                      data-testid={`reject-entity-${cand.entity_id}`}
                    >
                      <X className="h-3 w-3" />
                      {tx("拒绝", "Reject")}
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
