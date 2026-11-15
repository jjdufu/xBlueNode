import { useCallback, useMemo, useState } from "react";
import {
  Background,
  BaseEdge,
  Controls,
  EdgeLabelRenderer,
  MarkerType,
  MiniMap,
  ReactFlow,
  getSmoothStepPath,
  type Connection,
  type Edge,
  type EdgeProps,
  type EdgeTypes,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs/lib/elk.bundled.js";
import { LayoutGrid, Link2, Plus } from "lucide-react";
import type { Cardinality } from "@/lib/api/enums";
import type {
  EdgeTypeSpec,
  NodeTypeSpec,
  ValidationIssue,
} from "@/lib/api/xblue";
import { Button, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  StudioCanvasNodeCard,
  type StudioFlowNode,
} from "./NodeTypeCard";

export interface StudioEdgeData extends Record<string, unknown> {
  spec: EdgeTypeSpec;
  edgeIndex: number;
  selected: boolean;
  onSelectEdge: (index: number) => void;
}

export type StudioFlowEdge = Edge<StudioEdgeData, "studioEdge">;

const CARDINALITY_SHORT: Record<Cardinality, string> = {
  ONE_TO_ONE: "1:1",
  ONE_TO_MANY: "1:N",
  MANY_TO_ONE: "N:1",
  MANY_TO_MANY: "M:N",
};

function StudioRelationshipEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  data,
}: EdgeProps<StudioFlowEdge>) {
  const [edgePath, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    borderRadius: 8,
  });

  const spec = data?.spec;
  const selected = Boolean(data?.selected);
  const badgeText = spec
    ? CARDINALITY_SHORT[spec.cardinality] ?? "M:N"
    : "M:N";

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: selected ? "#2563eb" : "#71717a",
          strokeWidth: selected ? 2.2 : 1.5,
        }}
      />
      {spec && (
        <EdgeLabelRenderer>
          <div
            style={{
              position: "absolute",
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
              pointerEvents: "all",
            }}
            className="nodrag nopan"
          >
            <button
              type="button"
              data-testid={`canvas-edge-${spec.name}`}
              onClick={(e) => {
                e.stopPropagation();
                data?.onSelectEdge(data.edgeIndex);
              }}
              className={cn(
                "inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs transition-colors cursor-pointer",
                selected
                  ? "border-xblue bg-xblue text-white"
                  : "border-border bg-card text-foreground hover:border-xblue"
              )}
            >
              <span className="font-semibold">{spec.name}</span>
              <span
                className={cn(
                  "rounded px-1 text-[9px]",
                  selected
                    ? "bg-white/20 text-white"
                    : "bg-muted text-muted-foreground"
                )}
              >
                {badgeText}
              </span>
            </button>
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

const NODE_TYPES: NodeTypes = {
  studioNode: StudioCanvasNodeCard,
};

const EDGE_TYPES: EdgeTypes = {
  studioEdge: StudioRelationshipEdge,
};

export interface StudioCanvasProps {
  nodeTypes: NodeTypeSpec[];
  edgeTypes: EdgeTypeSpec[];
  issues: ValidationIssue[];
  selectedNodeName: string | null;
  selectedEdgeIndex: number | null;
  onSelectNode: (name: string | null) => void;
  onSelectEdge: (edgeIndex: number | null) => void;
  onUpdateNodePosition: (name: string, posX: number, posY: number) => void;
  onApplyLayoutPositions: (positions: Record<string, { x: number; y: number }>) => void;
  onCreateNodeType: (name: string, displayName: string) => void;
  onCreateEdgeType: (edge: EdgeTypeSpec) => void;
}

export function StudioCanvas({
  nodeTypes,
  edgeTypes,
  issues,
  selectedNodeName,
  selectedEdgeIndex,
  onSelectNode,
  onSelectEdge,
  onUpdateNodePosition,
  onApplyLayoutPositions,
  onCreateNodeType,
  onCreateEdgeType,
}: StudioCanvasProps) {
  const [quickAddNodeOpen, setQuickAddNodeOpen] = useState(false);
  const [newNodeName, setNewNodeName] = useState("");
  const [newNodeDisplay, setNewNodeDisplay] = useState("");
  const [quickAddEdgeOpen, setQuickAddEdgeOpen] = useState(false);
  const [newEdgeName, setNewEdgeName] = useState("SUPPLIES");
  const [newEdgeSrc, setNewEdgeSrc] = useState("");
  const [newEdgeDst, setNewEdgeDst] = useState("");
  const [layoutRunning, setLayoutRunning] = useState(false);

  const issueStatsByNode = useMemo(() => {
    const stats: Record<string, { count: number; hasError: boolean }> = {};
    for (const issue of issues) {
      const match = /^node_types\[(\d+)\]/.exec(issue.path);
      if (match) {
        const idx = Number(match[1]);
        const target = nodeTypes[idx];
        if (target) {
          const prev = stats[target.name] ?? { count: 0, hasError: false };
          stats[target.name] = {
            count: prev.count + 1,
            hasError: prev.hasError || issue.severity === "ERROR",
          };
        }
      }
    }
    return stats;
  }, [issues, nodeTypes]);

  const flowNodes: StudioFlowNode[] = useMemo(
    () =>
      nodeTypes.map((nt, idx) => {
        const st = issueStatsByNode[nt.name] ?? { count: 0, hasError: false };
        return {
          id: nt.name,
          type: "studioNode",
          position: {
            x: Number.isFinite(nt.pos_x) ? nt.pos_x : 100 + idx * 280,
            y: Number.isFinite(nt.pos_y) ? nt.pos_y : 120,
          },
          data: {
            spec: nt,
            selected: selectedNodeName === nt.name,
            issueCount: st.count,
            hasError: st.hasError,
          },
        };
      }),
    [nodeTypes, selectedNodeName, issueStatsByNode]
  );

  const nodeNameSet = useMemo(
    () => new Set(nodeTypes.map((n) => n.name)),
    [nodeTypes]
  );

  const flowEdges: StudioFlowEdge[] = useMemo(
    () =>
      edgeTypes
        .map((et, idx) => ({ et, idx }))
        .filter(
          ({ et }) =>
            nodeNameSet.has(et.src_type_name) &&
            nodeNameSet.has(et.dst_type_name)
        )
        .map(({ et, idx }) => ({
          id: `edge-${idx}-${et.src_type_name}-${et.name}-${et.dst_type_name}`,
          source: et.src_type_name,
          target: et.dst_type_name,
          type: "studioEdge",
          markerEnd: {
            type: MarkerType.ArrowClosed,
            color: selectedEdgeIndex === idx ? "#2563eb" : "#71717a",
          },
          data: {
            spec: et,
            edgeIndex: idx,
            selected: selectedEdgeIndex === idx,
            onSelectEdge: (edgeIdx: number) => {
              onSelectNode(null);
              onSelectEdge(edgeIdx);
            },
          },
        })),
    [edgeTypes, nodeNameSet, selectedEdgeIndex, onSelectEdge, onSelectNode]
  );

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target) return;
      const candidateBase = `${connection.source.toUpperCase()}_TO_${connection.target.toUpperCase()}`;
      onCreateEdgeType({
        name: candidateBase.slice(0, 32),
        src_type_name: connection.source,
        dst_type_name: connection.target,
        impact_weight: 0.7,
        cardinality: "ONE_TO_MANY",
        description: "",
        properties: [],
        propagation: {},
      });
    },
    [onCreateEdgeType]
  );

  const runElkLayout = async () => {
    if (nodeTypes.length === 0 || layoutRunning) return;
    setLayoutRunning(true);
    try {
      const elk = new ELK();
      const graph = await elk.layout({
        id: "studio-root",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "RIGHT",
          "elk.layered.spacing.nodeNodeBetweenLayers": "120",
          "elk.spacing.nodeNode": "72",
          "elk.padding": "[top=48,left=48,bottom=48,right=48]",
        },
        children: nodeTypes.map((nt) => ({
          id: nt.name,
          width: 224,
          height: 136,
        })),
        edges: edgeTypes
          .filter(
            (et) =>
              nodeNameSet.has(et.src_type_name) &&
              nodeNameSet.has(et.dst_type_name)
          )
          .map((et, idx) => ({
            id: `elk-e-${idx}`,
            sources: [et.src_type_name],
            targets: [et.dst_type_name],
          })),
      });

      const positions: Record<string, { x: number; y: number }> = {};
      for (const child of graph.children ?? []) {
        if (child.id && typeof child.x === "number" && typeof child.y === "number") {
          positions[child.id] = { x: Math.round(child.x), y: Math.round(child.y) };
        }
      }
      onApplyLayoutPositions(positions);
    } catch {
      // Fallback deterministic grid layout if ELK encounters an edge case
      const fallback: Record<string, { x: number; y: number }> = {};
      nodeTypes.forEach((nt, idx) => {
        fallback[nt.name] = {
          x: 80 + (idx % 3) * 290,
          y: 80 + Math.floor(idx / 3) * 190,
        };
      });
      onApplyLayoutPositions(fallback);
    } finally {
      setLayoutRunning(false);
    }
  };

  const submitQuickNode = () => {
    const trimmedName = newNodeName.trim();
    const trimmedDisplay = newNodeDisplay.trim();
    if (!trimmedName || !trimmedDisplay) return;
    onCreateNodeType(trimmedName, trimmedDisplay);
    setNewNodeName("");
    setNewNodeDisplay("");
    setQuickAddNodeOpen(false);
  };

  const submitQuickEdge = () => {
    const src = newEdgeSrc || nodeTypes[0]?.name || "";
    const dst = newEdgeDst || nodeTypes[1]?.name || nodeTypes[0]?.name || "";
    const name = newEdgeName.trim();
    if (!src || !dst || !name) return;
    onCreateEdgeType({
      name,
      src_type_name: src,
      dst_type_name: dst,
      impact_weight: 0.7,
      cardinality: "ONE_TO_MANY",
      description: "",
      properties: [],
      propagation: {},
    });
    setQuickAddEdgeOpen(false);
  };

  return (
    <div
      className="relative h-[560px] w-full rounded-md border border-border bg-muted/20 overflow-hidden"
      data-testid="xbluestudio-canvas"
    >
      {/* Top Floating Canvas Toolbar */}
      <div className="absolute top-2.5 left-2.5 right-2.5 z-10 flex flex-wrap items-center justify-between gap-2 pointer-events-none">
        <div className="flex flex-wrap items-center gap-1.5 pointer-events-auto rounded-md border border-border bg-card/95 px-2 py-1.5 shadow-xs">
          <Button
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="canvas-add-node-btn"
            onClick={() => {
              setQuickAddNodeOpen((v) => !v);
              setQuickAddEdgeOpen(false);
            }}
          >
            <Plus className="h-3.5 w-3.5" />
            新增节点类型
          </Button>
          <Button
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="canvas-add-edge-btn"
            onClick={() => {
              setNewEdgeSrc(nodeTypes[0]?.name ?? "");
              setNewEdgeDst(nodeTypes[1]?.name ?? nodeTypes[0]?.name ?? "");
              setQuickAddEdgeOpen((v) => !v);
              setQuickAddNodeOpen(false);
            }}
          >
            <Link2 className="h-3.5 w-3.5" />
            新增关系连线
          </Button>
          <Button
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="canvas-elk-layout-btn"
            onClick={() => void runElkLayout()}
            disabled={layoutRunning || nodeTypes.length === 0}
          >
            <LayoutGrid className="h-3.5 w-3.5 text-xblue" />
            {layoutRunning ? "布局计算中..." : "ELK 自动分层布局"}
          </Button>
        </div>

        <div className="pointer-events-auto rounded-md border border-border bg-card/95 px-2.5 py-1 text-[11px] font-mono-tech text-muted-foreground">
          类型 {nodeTypes.length} · 关系 {edgeTypes.length} · 拖拽右侧锚点可连线
        </div>
      </div>

      {/* Quick Create Node Popover */}
      {quickAddNodeOpen && (
        <div className="absolute top-14 left-2.5 z-20 w-80 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-medium">新建业务节点类型</div>
          <div className="grid grid-cols-2 gap-1.5">
            <Input
              data-testid="quick-node-name-input"
              placeholder="英文名 (如 Factory)"
              value={newNodeName}
              onChange={(e) => setNewNodeName(e.target.value)}
              className="h-8 text-xs font-mono-tech"
            />
            <Input
              data-testid="quick-node-display-input"
              placeholder="中文名 (如 制造工厂)"
              value={newNodeDisplay}
              onChange={(e) => setNewNodeDisplay(e.target.value)}
              className="h-8 text-xs"
            />
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              variant="ghost"
              className="h-7 px-2 py-1"
              onClick={() => setQuickAddNodeOpen(false)}
            >
              取消
            </Button>
            <Button
              data-testid="quick-node-submit-btn"
              className="h-7 px-2.5 py-1"
              onClick={submitQuickNode}
            >
              创建
            </Button>
          </div>
        </div>
      )}

      {/* Quick Create Edge Popover */}
      {quickAddEdgeOpen && (
        <div className="absolute top-14 left-2.5 z-20 w-96 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-medium">新建因果/业务关系类型</div>
          <div className="grid grid-cols-3 gap-1.5">
            <select
              value={newEdgeSrc}
              onChange={(e) => setNewEdgeSrc(e.target.value)}
              className="h-8 rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
            >
              <option value="">源类型</option>
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.name}
                </option>
              ))}
            </select>
            <Input
              data-testid="quick-edge-name-input"
              value={newEdgeName}
              onChange={(e) => setNewEdgeName(e.target.value)}
              placeholder="如 FULFILLS"
              className="h-8 text-xs font-mono-tech"
            />
            <select
              value={newEdgeDst}
              onChange={(e) => setNewEdgeDst(e.target.value)}
              className="h-8 rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
            >
              <option value="">目标类型</option>
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.name}
                </option>
              ))}
            </select>
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              variant="ghost"
              className="h-7 px-2 py-1"
              onClick={() => setQuickAddEdgeOpen(false)}
            >
              取消
            </Button>
            <Button
              data-testid="quick-edge-submit-btn"
              className="h-7 px-2.5 py-1"
              onClick={submitQuickEdge}
            >
              创建关系
            </Button>
          </div>
        </div>
      )}

      <ReactFlow
        nodes={flowNodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onNodeClick={(_event, node) => {
          onSelectEdge(null);
          onSelectNode(node.id);
        }}
        onEdgeClick={(_event, edge) => {
          if (typeof edge.data?.edgeIndex === "number") {
            onSelectNode(null);
            onSelectEdge(edge.data.edgeIndex);
          }
        }}
        onPaneClick={() => {
          onSelectNode(null);
          onSelectEdge(null);
        }}
        onNodeDragStop={(_event, node) => {
          onUpdateNodePosition(
            node.id,
            Math.round(node.position.x),
            Math.round(node.position.y)
          );
        }}
        onConnect={handleConnect}
        fitView
        fitViewOptions={{ padding: 0.22, maxZoom: 1.1 }}
        proOptions={{ hideAttribution: true }}
      >
        <Background gap={18} size={1} />
        <Controls showInteractive={false} />
        <MiniMap
          pannable
          zoomable
          className="!bg-card !border !border-border !rounded"
        />
      </ReactFlow>

      {/* Non-anchored cross-subgraph edges indicator (if dst_type_name lives in main while editing a dept subgraph) */}
      {edgeTypes.length > flowEdges.length && (
        <div className="absolute bottom-2.5 left-12 z-10 flex flex-wrap items-center gap-1 rounded border border-border bg-card/95 px-2.5 py-1 text-[11px]">
          <span className="text-muted-foreground">跨子图引用关系:</span>
          {edgeTypes
            .map((et, idx) => ({ et, idx }))
            .filter(
              ({ et }) =>
                !nodeNameSet.has(et.src_type_name) ||
                !nodeNameSet.has(et.dst_type_name)
            )
            .map(({ et, idx }) => (
              <button
                key={`${et.name}-${idx}`}
                type="button"
                data-testid={`canvas-edge-${et.name}`}
                onClick={() => {
                  onSelectNode(null);
                  onSelectEdge(idx);
                }}
                className={cn(
                  "rounded border px-1.5 py-0.5 font-mono-tech text-[10px] cursor-pointer",
                  selectedEdgeIndex === idx
                    ? "border-xblue bg-xblue-soft text-xblue"
                    : "border-border bg-muted text-foreground"
                )}
              >
                {et.src_type_name} → {et.name} (
                {CARDINALITY_SHORT[et.cardinality]}) → {et.dst_type_name}
              </button>
            ))}
        </div>
      )}
    </div>
  );
}
