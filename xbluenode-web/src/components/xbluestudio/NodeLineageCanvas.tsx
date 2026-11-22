import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ArrowLeft, GitBranch, Loader2, RefreshCw } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  computeNodeBadgeCounts,
  xblueApi,
  type LineageEdge,
  type LineageStage,
  type LineageStageKind,
  type LineageStageNode,
  type NodeLineageGraph,
  type SchemaDoc,
} from "@/lib/api/xblue";
import { Badge, Button, Card, IconButton, Select } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { renderNodeIcon } from "./NodeTypeCard";

export interface NodeLineageCanvasProps {
  subgraphSlug: string;
  nodeType: string;
  activeDraftId?: string | null;
  workingDoc: SchemaDoc;
  onSelectNodeType: (nodeType: string) => void;
  onBackToCanvas: () => void;
}

const STAGE_ORDER: readonly LineageStageKind[] = [
  "DATASET_COLUMNS",
  "MAPPING_SPEC",
  "NODE_SCHEMA",
  "COMPUTE_RULES",
  "DOWNSTREAM_CONSUMERS",
];

interface RenderedCurve {
  readonly key: string;
  readonly from: string;
  readonly to: string;
  readonly label: string;
  readonly path: string;
  readonly midX: number;
  readonly midY: number;
}

function expressionMentionsIdent(expr: string, ident: string): boolean {
  if (!ident) return false;
  const escaped = ident.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?<![A-Za-z0-9_])${escaped}(?![A-Za-z0-9_])`).test(expr);
}

/** Synthesizes deterministic 5-stage lineage from `workingDoc` when offline or inspecting an unsaved node type. */
function buildLocalFallbackLineage(
  subgraphSlug: string,
  nodeType: string,
  doc: SchemaDoc
): NodeLineageGraph {
  const nt = doc.node_types.find((item) => item.name === nodeType);
  if (!nt) {
    return {
      subgraph_slug: subgraphSlug,
      node_type: nodeType,
      display_name: nodeType,
      badge_counts: { property_count: 0, metric_count: 0, rule_count: 0 },
      stages: STAGE_ORDER.map((stage) => ({ stage, nodes: [] })),
      edges: [],
    };
  }

  const stage1: LineageStageNode[] = [];
  const stage1Seen = new Set<string>();
  const stage2: LineageStageNode[] = [];
  const stage3: LineageStageNode[] = [];
  const stage3Seen = new Set<string>();
  const stage4: LineageStageNode[] = [];
  const edges: LineageEdge[] = [];

  const ensureCol = (dataset: string, col: string): string => {
    const id = `col:${dataset}.${col}`;
    if (!stage1Seen.has(id)) {
      stage1Seen.add(id);
      stage1.push({
        id,
        label: `${dataset}.${col}`,
        meta: { dataset, column: col, data_type: "STRING" },
      });
    }
    return id;
  };

  const keyId = `key:${nt.name}.node_key`;
  stage3Seen.add(keyId);
  stage3.push({
    id: keyId,
    label: "node_key",
    meta: { kind: "PRIMARY_KEY", maturity: nt.maturity ?? "CONFIRMED" },
  });

  for (const prop of nt.properties) {
    const pid = `prop:${nt.name}.${prop.name}`;
    stage3Seen.add(pid);
    stage3.push({
      id: pid,
      label: prop.unit ? `${prop.name} (${prop.unit})` : prop.name,
      meta: {
        kind: prop.kind,
        data_type: prop.data_type,
        maturity: prop.maturity ?? "CONFIRMED",
        display_name: prop.display_name,
      },
    });
  }

  for (const mapping of doc.mappings.filter((m) => m.node_type === nt.name)) {
    const mapId = `map:${mapping.name}`;
    stage2.push({
      id: mapId,
      label: `${mapping.name} (${mapping.mode})`,
      meta: {
        dataset: mapping.dataset,
        key_column: mapping.key_column,
        mode: mapping.mode,
      },
    });
    if (mapping.dataset && mapping.key_column) {
      const colId = ensureCol(mapping.dataset, mapping.key_column);
      edges.push({ from: colId, to: mapId, label: "KEY" });
      edges.push({ from: mapId, to: keyId, label: "SYNC" });
    }
    for (const [attrName, srcCol] of Object.entries(mapping.attributes ?? {})) {
      if (!srcCol) continue;
      const colId = ensureCol(mapping.dataset, srcCol);
      edges.push({ from: colId, to: mapId, label: "DIRECT" });
      edges.push({
        from: mapId,
        to: `prop:${nt.name}.${attrName}`,
        label: "SYNC",
      });
    }
    for (const [metricName, srcCol] of Object.entries(mapping.metrics ?? {})) {
      if (!srcCol) continue;
      const colId = ensureCol(mapping.dataset, srcCol);
      edges.push({ from: colId, to: mapId, label: "DIRECT" });
      edges.push({
        from: mapId,
        to: `prop:${nt.name}.${metricName}`,
        label: "SYNC",
      });
    }
  }

  nt.agg_rules.forEach((agg, idx) => {
    const ruleId = `agg:${nt.name}.${agg.target_metric || idx}`;
    stage4.push({
      id: ruleId,
      label: `${agg.op}(${agg.source_type_name}.${agg.source_metric} via ${agg.via_link_type})`,
      meta: {
        kind: "AGG",
        op: agg.op,
        maturity: agg.maturity ?? "CONFIRMED",
      },
    });
    if (agg.source_type_name && agg.source_metric) {
      const upstreamId = `upstream:${agg.source_type_name}.${agg.source_metric}`;
      if (!stage3Seen.has(upstreamId)) {
        stage3Seen.add(upstreamId);
        stage3.push({
          id: upstreamId,
          label: `${agg.source_type_name}.${agg.source_metric}`,
          meta: { kind: "UPSTREAM_METRIC", source_type: agg.source_type_name },
        });
      }
      edges.push({ from: upstreamId, to: ruleId, label: "INPUT" });
    }
    if (agg.target_metric) {
      edges.push({
        from: ruleId,
        to: `prop:${nt.name}.${agg.target_metric}`,
        label: "AGG",
      });
    }
  });

  nt.derived_rules.forEach((der, idx) => {
    const ruleId = `derived:${nt.name}.${der.target_metric || idx}`;
    stage4.push({
      id: ruleId,
      label: der.expression,
      meta: {
        kind: "DERIVED",
        enabled: der.enabled,
        maturity: der.maturity ?? "CONFIRMED",
      },
    });
    for (const prop of nt.properties) {
      if (
        prop.name !== der.target_metric &&
        expressionMentionsIdent(der.expression, prop.name)
      ) {
        edges.push({
          from: `prop:${nt.name}.${prop.name}`,
          to: ruleId,
          label: "INPUT",
        });
      }
    }
    if (der.target_metric) {
      edges.push({
        from: ruleId,
        to: `prop:${nt.name}.${der.target_metric}`,
        label: "DERIVED",
      });
    }
  });

  return {
    subgraph_slug: subgraphSlug,
    node_type: nt.name,
    display_name: nt.display_name || nt.name,
    badge_counts: computeNodeBadgeCounts(nt),
    stages: [
      { stage: "DATASET_COLUMNS", nodes: stage1 },
      { stage: "MAPPING_SPEC", nodes: stage2 },
      { stage: "NODE_SCHEMA", nodes: stage3 },
      { stage: "COMPUTE_RULES", nodes: stage4 },
      { stage: "DOWNSTREAM_CONSUMERS", nodes: [] },
    ],
    edges,
  };
}

export function NodeLineageCanvas({
  subgraphSlug,
  nodeType,
  activeDraftId,
  workingDoc,
  onSelectNodeType,
  onBackToCanvas,
}: NodeLineageCanvasProps) {
  const { tx, tEnum } = useI18n();
  const [graph, setGraph] = useState<NodeLineageGraph | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [curves, setCurves] = useState<RenderedCurve[]>([]);

  const boardRef = useRef<HTMLDivElement | null>(null);
  const itemRefs = useRef<Map<string, HTMLDivElement>>(new Map());

  const loadLineage = useCallback(async () => {
    if (!nodeType) return;
    setLoading(true);
    try {
      const res = await xblueApi.getNodeLineage(
        subgraphSlug,
        nodeType,
        activeDraftId ?? undefined
      );
      if (res && Array.isArray(res.stages) && res.stages.length > 0) {
        setGraph(res);
      } else {
        setGraph(buildLocalFallbackLineage(subgraphSlug, nodeType, workingDoc));
      }
    } catch {
      setGraph(buildLocalFallbackLineage(subgraphSlug, nodeType, workingDoc));
    } finally {
      setLoading(false);
    }
  }, [subgraphSlug, nodeType, activeDraftId, workingDoc]);

  useEffect(() => {
    setHoveredId(null);
    void loadLineage();
  }, [loadLineage]);

  const effectiveGraph = useMemo(
    () =>
      graph ?? buildLocalFallbackLineage(subgraphSlug, nodeType, workingDoc),
    [graph, subgraphSlug, nodeType, workingDoc]
  );

  const stageMap = useMemo(() => {
    const map = new Map<LineageStageKind, LineageStage>();
    for (const s of effectiveGraph.stages) {
      map.set(s.stage, s);
    }
    return map;
  }, [effectiveGraph.stages]);

  const recomputeCurves = useCallback(() => {
    const board = boardRef.current;
    if (!board) return;
    const boardRect = board.getBoundingClientRect();
    if (boardRect.width <= 0 || boardRect.height <= 0) return;

    const nextCurves: RenderedCurve[] = [];
    for (let i = 0; i < effectiveGraph.edges.length; i += 1) {
      const edge = effectiveGraph.edges[i];
      if (!edge) continue;
      const srcEl = itemRefs.current.get(edge.from);
      const dstEl = itemRefs.current.get(edge.to);
      if (!srcEl || !dstEl) continue;

      const srcRect = srcEl.getBoundingClientRect();
      const dstRect = dstEl.getBoundingClientRect();
      if (srcRect.width <= 0 || dstRect.width <= 0) continue;

      const forward = srcRect.left <= dstRect.left;
      const x1 = forward
        ? srcRect.right - boardRect.left + board.scrollLeft
        : srcRect.left - boardRect.left + board.scrollLeft;
      const y1 =
        srcRect.top -
        boardRect.top +
        board.scrollTop +
        srcRect.height / 2 +
        (forward ? 0 : 4);
      const x2 = forward
        ? dstRect.left - boardRect.left + board.scrollLeft
        : dstRect.right - boardRect.left + board.scrollLeft;
      const y2 =
        dstRect.top -
        boardRect.top +
        board.scrollTop +
        dstRect.height / 2 +
        (forward ? 0 : 4);

      const dx = Math.max(28, Math.abs(x2 - x1) * 0.44);
      const cx1 = forward ? x1 + dx : x1 - dx;
      const cx2 = forward ? x2 - dx : x2 + dx;

      nextCurves.push({
        key: `${edge.from}->${edge.to}#${i}`,
        from: edge.from,
        to: edge.to,
        label: edge.label,
        path: `M ${x1.toFixed(1)} ${y1.toFixed(1)} C ${cx1.toFixed(1)} ${y1.toFixed(1)}, ${cx2.toFixed(1)} ${y2.toFixed(1)}, ${x2.toFixed(1)} ${y2.toFixed(1)}`,
        midX: (x1 + x2) / 2,
        midY: (y1 + y2) / 2,
      });
    }
    setCurves(nextCurves);
  }, [effectiveGraph.edges]);

  useLayoutEffect(() => {
    recomputeCurves();
  }, [recomputeCurves, effectiveGraph]);

  useEffect(() => {
    const board = boardRef.current;
    if (!board || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => recomputeCurves());
    ro.observe(board);
    return () => {
      ro.disconnect();
    };
  }, [recomputeCurves]);

  const { connectedNodes, connectedEdgeKeys } = useMemo(() => {
    const nodes = new Set<string>();
    const edgeKeys = new Set<string>();
    if (!hoveredId) {
      return { connectedNodes: nodes, connectedEdgeKeys: edgeKeys };
    }
    nodes.add(hoveredId);

    const upQueue: string[] = [hoveredId];
    const visitedUp = new Set<string>([hoveredId]);
    while (upQueue.length > 0) {
      const cur = upQueue.shift();
      if (!cur) continue;
      effectiveGraph.edges.forEach((e, idx) => {
        if (e.to === cur) {
          edgeKeys.add(`${e.from}->${e.to}#${idx}`);
          nodes.add(e.from);
          if (!visitedUp.has(e.from)) {
            visitedUp.add(e.from);
            upQueue.push(e.from);
          }
        }
      });
    }

    const downQueue: string[] = [hoveredId];
    const visitedDown = new Set<string>([hoveredId]);
    while (downQueue.length > 0) {
      const cur = downQueue.shift();
      if (!cur) continue;
      effectiveGraph.edges.forEach((e, idx) => {
        if (e.from === cur) {
          edgeKeys.add(`${e.from}->${e.to}#${idx}`);
          nodes.add(e.to);
          if (!visitedDown.has(e.to)) {
            visitedDown.add(e.to);
            downQueue.push(e.to);
          }
        }
      });
    }

    return { connectedNodes: nodes, connectedEdgeKeys: edgeKeys };
  }, [hoveredId, effectiveGraph.edges]);

  const stageTitle = (kind: LineageStageKind): string => {
    switch (kind) {
      case "DATASET_COLUMNS":
        return tx("源数据集列", "Dataset Columns");
      case "MAPPING_SPEC":
        return tx("映射规格", "Mapping Specs");
      case "NODE_SCHEMA":
        return tx("节点属性与指标", "Properties & Metrics");
      case "COMPUTE_RULES":
        return tx("聚合与派生", "Rollups & Formulas");
      case "DOWNSTREAM_CONSUMERS":
        return tx("规则/端点/驾驶舱", "Consumers");
    }
  };

  const renderMetaBadge = (node: LineageStageNode) => {
    const kind =
      typeof node.meta.kind === "string"
        ? node.meta.kind
        : typeof node.meta.consumer_kind === "string"
          ? node.meta.consumer_kind
          : typeof node.meta.data_type === "string"
            ? node.meta.data_type
            : null;
    const maturity =
      typeof node.meta.maturity === "string" ? node.meta.maturity : null;
    return (
      <div className="flex items-center gap-1 shrink-0">
        {maturity === "PROPOSED" && (
          <Badge variant="warning" className="text-[10px]">
            {tEnum("PROPOSED")}
          </Badge>
        )}
        {kind && (
          <Badge
            variant={
              kind === "METRIC" || kind === "AGG" || kind === "DERIVED"
                ? "xblue"
                : "outline"
            }
            className="text-[10px]"
          >
            {kind}
          </Badge>
        )}
      </div>
    );
  };

  const counts = effectiveGraph.badge_counts;

  return (
    <Card
      data-testid="node-lineage-canvas"
      className="flex-1 min-h-[360px] w-full flex flex-col overflow-hidden text-xs"
    >
      {/* Lineage Toolbar Header */}
      <div className="shrink-0 flex flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/20 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2 min-w-0">
          <GitBranch className="h-4 w-4 text-xblue shrink-0" />
          <span className="font-semibold text-sm">
            {tx("血缘视图", "Lineage")}
          </span>
          {workingDoc.node_types.length > 0 && (
            <Select
              data-testid="lineage-node-select"
              value={effectiveGraph.node_type}
              aria-label={tx("节点类型", "Node Type")}
              onChange={(e) => onSelectNodeType(e.target.value)}
              options={workingDoc.node_types.map((nt) => ({
                value: nt.name,
                label: `${nt.display_name || nt.name} (${nt.name})`,
                text: `${nt.display_name} ${nt.name}`,
                icon: renderNodeIcon(nt.icon),
              }))}
              className="h-7 w-52 font-mono-tech"
            />
          )}
          <Badge
            variant="neutral"
            data-testid="lineage-badge-counts"
            className="tabular-nums font-mono-tech"
          >
            {`${counts.property_count}p·${counts.metric_count}m·${counts.rule_count}r`}
          </Badge>
          {loading && (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-xblue" />
          )}
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          <IconButton
            size="sm"
            onClick={() => void loadLineage()}
            aria-label={tx("刷新血缘", "Refresh Lineage")}
            title={tx("刷新", "Refresh")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </IconButton>
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="lineage-back-canvas-btn"
            onClick={onBackToCanvas}
          >
            <ArrowLeft className="h-3.5 w-3.5" />
            {tx("返回画布", "Canvas")}
          </Button>
        </div>
      </div>

      {/* 5-Stage Horizontal Pipeline Board with SVG Connection Overlay */}
      <div
        ref={boardRef}
        onScroll={recomputeCurves}
        className="relative flex-1 min-h-0 overflow-auto p-4 bg-muted/10"
      >
        <svg
          data-testid="lineage-svg-edges"
          className="pointer-events-none absolute inset-0 h-full w-full min-w-[920px]"
        >
          <defs>
            <marker
              id="lineage-arrow-idle"
              markerWidth="7"
              markerHeight="7"
              refX="6"
              refY="3.5"
              orient="auto"
            >
              <path d="M0,0.5 L6,3.5 L0,6.5 Z" fill="#94a3b8" />
            </marker>
            <marker
              id="lineage-arrow-active"
              markerWidth="7"
              markerHeight="7"
              refX="6"
              refY="3.5"
              orient="auto"
            >
              <path d="M0,0.5 L6,3.5 L0,6.5 Z" fill="#2563eb" />
            </marker>
          </defs>
          {curves.map((curve) => {
            const active = hoveredId !== null && connectedEdgeKeys.has(curve.key);
            const dimmed = hoveredId !== null && !active;
            return (
              <g
                key={curve.key}
                className={cn(
                  "transition-opacity",
                  dimmed ? "opacity-20" : "opacity-100"
                )}
              >
                <path
                  d={curve.path}
                  fill="none"
                  stroke={active ? "#2563eb" : "#94a3b8"}
                  strokeWidth={active ? 2.2 : 1.4}
                  markerEnd={
                    active
                      ? "url(#lineage-arrow-active)"
                      : "url(#lineage-arrow-idle)"
                  }
                />
                {active && curve.label && (
                  <text
                    x={curve.midX}
                    y={curve.midY - 5}
                    textAnchor="middle"
                    fill="#2563eb"
                    fontSize="9"
                    fontWeight="600"
                  >
                    {curve.label}
                  </text>
                )}
              </g>
            );
          })}
        </svg>

        <div className="relative z-10 grid grid-cols-5 gap-6 min-w-[920px]">
          {STAGE_ORDER.map((stageKind, colIdx) => {
            const stage = stageMap.get(stageKind);
            const stageNodes = stage?.nodes ?? [];
            return (
              <div
                key={stageKind}
                data-testid={`lineage-stage-${stageKind}`}
                className="flex flex-col gap-2 min-w-0"
              >
                <div className="flex items-center justify-between gap-1.5 rounded-md border border-border bg-card px-2.5 py-1.5">
                  <span className="font-semibold truncate">
                    <span className="font-mono-tech text-muted-foreground mr-1">
                      {colIdx + 1}.
                    </span>
                    {stageTitle(stageKind)}
                  </span>
                  <span className="font-mono-tech text-[11px] text-muted-foreground tabular-nums">
                    {stageNodes.length}
                  </span>
                </div>

                {stageNodes.length === 0 ? (
                  <div className="rounded-md border border-dashed border-border bg-card/40 px-2.5 py-6 text-center text-[11px] text-muted-foreground">
                    {tx("暂无节点", "Empty")}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {stageNodes.map((item) => {
                      const highlighted =
                        hoveredId !== null && connectedNodes.has(item.id);
                      const dimmed = hoveredId !== null && !highlighted;
                      return (
                        <div
                          key={item.id}
                          ref={(el) => {
                            if (el) {
                              itemRefs.current.set(item.id, el);
                            } else {
                              itemRefs.current.delete(item.id);
                            }
                          }}
                          data-testid={`lineage-node-${item.id}`}
                          onMouseEnter={() => setHoveredId(item.id)}
                          onMouseLeave={() => setHoveredId(null)}
                          className={cn(
                            "rounded-md border bg-card p-2 shadow-2xs transition-all cursor-default space-y-1",
                            highlighted
                              ? "border-xblue text-xblue bg-xblue-soft/20 ring-1 ring-xblue/35"
                              : "border-border text-card-foreground hover:border-foreground/40",
                            dimmed && "opacity-35"
                          )}
                        >
                          <div className="font-mono-tech text-[11px] font-semibold break-all leading-snug">
                            {item.label}
                          </div>
                          <div className="flex items-center justify-between gap-1">
                            <span className="text-[10px] text-muted-foreground truncate font-mono-tech">
                              {typeof item.meta.display_name === "string" &&
                              item.meta.display_name
                                ? item.meta.display_name
                                : item.id.split(":")[0]}
                            </span>
                            {renderMetaBadge(item)}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </Card>
  );
}
