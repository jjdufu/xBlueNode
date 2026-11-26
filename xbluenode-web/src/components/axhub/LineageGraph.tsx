import { Suspense, lazy, useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { GitFork, Loader2, RefreshCw, X } from "lucide-react";
import { useI18n } from "@/i18n";
import { axhubApi, type Dataset, type LineageEdge } from "@/lib/api/axhub";
import { useTheme } from "@/lib/theme";
import { Badge, Button, Card, Select } from "@/components/ui/primitives";
import type {
  LineageAssetEdgeModel,
  LineageAssetNodeModel,
} from "./LineageGraphG6Surface";

const LazyLineageGraphG6Surface = lazy(
  () => import("./LineageGraphG6Surface")
);

export interface LineageGraphProps {
  dataset: Dataset;
  connectionNames?: Record<string, string>;
}

export function LineageGraph({ dataset, connectionNames }: LineageGraphProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const navigate = useNavigate();

  const [depth, setDepth] = useState(3);
  const [edges, setEdges] = useState<LineageEdge[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [focusedNodeId, setFocusedNodeId] = useState<string | null>(null);
  const [hideDownstreamLeafs, setHideDownstreamLeafs] = useState(false);

  const formatLineageKind = useCallback(
    (kind: string): string => {
      switch (kind) {
        case "CONNECTION":
          return tx("连接源", "Connection");
        case "UPLOAD":
          return tx("上传文件", "Upload");
        case "DATASET":
          return tx("数据集", "Dataset");
        case "DOC":
          return tx("知识文档", "Document");
        case "METRIC":
          return tx("语义指标", "Metric");
        case "NODE_TYPE":
          return tx("节点类型", "Node Type");
        case "FLOW":
          return tx("编排流程", "Flow");
        case "AGENT":
          return tx("智能体", "Agent");
        case "DASHBOARD":
          return tx("驾驶舱", "Dashboard");
        case "EXPORT":
          return tx("导出任务", "Export");
        case "DOWNSTREAM":
          return tx("下游资产", "Downstream");
        default:
          return tEnum(kind);
      }
    },
    [tEnum, tx]
  );

  const loadLineage = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axhubApi.getLineage(dataset.id, depth);
      setEdges(Array.isArray(res?.edges) ? res.edges : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [dataset.id, depth]);

  useEffect(() => {
    setFocusedNodeId(null);
    void loadLineage();
  }, [loadLineage]);

  const formatNodeTitle = useCallback(
    (kind: string, rawId: string): string => {
      if (kind === "DATASET" && rawId === dataset.id) {
        return dataset.name;
      }
      if (kind === "CONNECTION" && connectionNames?.[rawId]) {
        return connectionNames[rawId];
      }
      return rawId.length > 16 ? `${rawId.slice(0, 10)}…` : rawId;
    },
    [connectionNames, dataset.id, dataset.name]
  );

  const graphElements = useMemo(() => {
    const centerKey = `DATASET:${dataset.id}`;
    const filteredEdges = hideDownstreamLeafs
      ? edges.filter(
          (e) =>
            e.dst_kind !== "DASHBOARD" &&
            e.dst_kind !== "EXPORT" &&
            e.dst_kind !== "AGENT"
        )
      : edges;

    const nodeMap = new Map<string, LineageAssetNodeModel>();
    nodeMap.set(centerKey, {
      id: centerKey,
      kind: "DATASET",
      kindLabel: formatLineageKind("DATASET"),
      title: dataset.name,
      rawId: dataset.id,
      isCenter: true,
    });

    for (const edge of filteredEdges) {
      const srcKey = `${edge.src_kind}:${edge.src_id}`;
      if (!nodeMap.has(srcKey)) {
        nodeMap.set(srcKey, {
          id: srcKey,
          kind: edge.src_kind,
          kindLabel: formatLineageKind(edge.src_kind),
          title: formatNodeTitle(edge.src_kind, edge.src_id),
          rawId: edge.src_id,
          isCenter: srcKey === centerKey,
        });
      }

      const dstKey = `${edge.dst_kind}:${edge.dst_id}`;
      if (!nodeMap.has(dstKey)) {
        nodeMap.set(dstKey, {
          id: dstKey,
          kind: edge.dst_kind,
          kindLabel: formatLineageKind(edge.dst_kind),
          title: formatNodeTitle(edge.dst_kind, edge.dst_id),
          rawId: edge.dst_id,
          isCenter: dstKey === centerKey,
        });
      }
    }

    const links: LineageAssetEdgeModel[] = filteredEdges.map((edge) => ({
      id: String(edge.id),
      source: `${edge.src_kind}:${edge.src_id}`,
      target: `${edge.dst_kind}:${edge.dst_id}`,
      viaLabel: edge.via ? tEnum(edge.via) : "",
    }));

    return {
      nodes: Array.from(nodeMap.values()),
      links,
    };
  }, [
    dataset.id,
    dataset.name,
    edges,
    formatLineageKind,
    formatNodeTitle,
    hideDownstreamLeafs,
    tEnum,
  ]);

  const handleNavigateAsset = useCallback(
    (kind: string, rawId: string) => {
      if (kind === "DASHBOARD") {
        void navigate(`/view/${encodeURIComponent(rawId)}`);
      } else if (kind === "NODE_TYPE") {
        void navigate(`/studio?node=${encodeURIComponent(rawId)}`);
      } else if (kind === "FLOW") {
        void navigate("/flow");
      } else if (kind === "DATASET" && rawId !== dataset.id) {
        void navigate(`/hub?dataset=${encodeURIComponent(rawId)}`);
      }
    },
    [dataset.id, navigate]
  );

  return (
    <Card className="p-4 space-y-3" data-testid="lineage-graph-panel">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2">
          <GitFork className="h-4 w-4 text-xblue" />
          <h3 className="text-xs font-semibold">
            {tx("血缘拓扑", "Lineage Graph")} · {dataset.name}
          </h3>
          {focusedNodeId && (
            <button
              type="button"
              data-testid="lineage-clear-focus-btn"
              onClick={() => setFocusedNodeId(null)}
              className="inline-flex items-center gap-1 rounded border border-xblue/40 bg-xblue-soft px-1.5 py-0.5 text-[10px] text-xblue cursor-pointer"
            >
              <span>{tx("闭包追踪", "Closure")}</span>
              <X className="h-2.5 w-2.5" />
            </button>
          )}
        </div>

        <div className="flex items-center gap-2">
          {edges.length > 0 && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="lineage-fold-downstream-btn"
              onClick={() => setHideDownstreamLeafs((v) => !v)}
            >
              {hideDownstreamLeafs
                ? tx("展开下游", "Expand Leaf")
                : tx("收纳下游", "Fold Leaf")}
            </Button>
          )}
          <label className="text-xs text-muted-foreground">
            {tx("跳数", "Hops")}
          </label>
          <Select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="h-8"
            data-testid="lineage-depth-select"
          >
            {[1, 2, 3, 4, 5].map((d) => (
              <option key={d} value={d}>
                {tx(`${d} 跳`, `${d} hops`)}
              </option>
            ))}
          </Select>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void loadLineage()}
            disabled={loading}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
            {tx("刷新", "Refresh")}
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-600 dark:text-red-400">
          {errorMsg}
        </div>
      )}

      <div
        className={`${
          graphElements.links.length === 0 ? "h-44" : "h-72"
        } w-full rounded border border-border bg-background overflow-hidden`}
        data-testid="lineage-graph-canvas"
      >
        <Suspense
          fallback={
            <div className="h-full w-full flex items-center justify-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin text-xblue" />
              <span>{tx("加载血缘画布…", "Loading lineage…")}</span>
            </div>
          }
        >
          <LazyLineageGraphG6Surface
            nodes={graphElements.nodes}
            edges={graphElements.links}
            focusedNodeId={focusedNodeId}
            isDark={isDark}
            onSelectNode={setFocusedNodeId}
            onNavigateAsset={handleNavigateAsset}
          />
        </Suspense>
      </div>

      <div className="space-y-1.5">
        <div className="text-[11px] font-medium text-muted-foreground">
          {tx(
            `${fmtNum(edges.length, 0)} 条关联边 · ${fmtNum(graphElements.nodes.length, 0)} 个节点`,
            `${fmtNum(edges.length, 0)} edges · ${fmtNum(graphElements.nodes.length, 0)} nodes`
          )}
        </div>
        {edges.length === 0 ? (
          <div className="text-xs text-muted-foreground">
            {tx("暂无上下游关联边", "No upstream or downstream edges")}
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5 text-xs">
            {edges.map((edge) => (
              <div
                key={edge.id}
                className="rounded border border-border px-2.5 py-1.5 flex items-center justify-between font-mono-tech text-[11px]"
                data-testid={`lineage-edge-${edge.id}`}
              >
                <span className="truncate">
                  <Badge variant="outline" className="mr-1">
                    {formatLineageKind(edge.src_kind)}
                  </Badge>
                  {formatNodeTitle(edge.src_kind, edge.src_id)} →{" "}
                  <Badge variant="xblue" className="mx-1">
                    {formatLineageKind(edge.dst_kind)}
                  </Badge>
                  {formatNodeTitle(edge.dst_kind, edge.dst_id)}
                </span>
                {edge.via && (
                  <span className="text-muted-foreground ml-2 shrink-0">
                    {tEnum(edge.via)}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}
