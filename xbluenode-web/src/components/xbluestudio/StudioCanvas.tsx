import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BaseEdge,
  Controls,
  EdgeLabelRenderer,
  MarkerType,
  MiniMap,
  ReactFlow,
  applyNodeChanges,
  getSmoothStepPath,
  type Connection,
  type Edge,
  type EdgeProps,
  type EdgeTypes,
  type NodeTypes,
  type OnNodesChange,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs/lib/elk.bundled.js";
import { LayoutGrid, Link2, Plus } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import type { Cardinality } from "@/lib/api/enums";
import type {
  EdgeTypeSpec,
  NodeTypeSpec,
  ValidationIssue,
} from "@/lib/api/xblue";
import { Button, Input, Select } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  StudioCanvasNodeCard,
  type StudioFlowNode,
} from "./NodeTypeCard";

export interface StudioEdgeData extends Record<string, unknown> {
  spec: EdgeTypeSpec;
  edgeIndex: number;
  selected: boolean;
  isDark: boolean;
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
    borderRadius: 10,
    offset: 24,
  });

  const spec = data?.spec;
  const selected = Boolean(data?.selected);
  const isDark = Boolean(data?.isDark);
  const edgeIndex = data?.edgeIndex ?? 0;
  const staggerY = edgeIndex % 3 === 0 ? -10 : edgeIndex % 3 === 1 ? 10 : 0;
  const badgeText = spec
    ? CARDINALITY_SHORT[spec.cardinality] ?? "M:N"
    : "M:N";
  const strokeColor = selected
    ? isDark
      ? "#3b82f6"
      : "#2563eb"
    : isDark
      ? "#71717a"
      : "#94a3b8";

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: strokeColor,
          strokeWidth: selected ? 2.2 : 1.6,
        }}
      />
      {spec && (
        <EdgeLabelRenderer>
          <div
            style={{
              position: "absolute",
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY + staggerY}px)`,
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
                "inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs backdrop-blur-xs transition-colors cursor-pointer",
                selected
                  ? "border-xblue bg-xblue text-white"
                  : "border-border bg-card/95 text-foreground hover:border-xblue"
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
  onApplyLayoutPositions: (
    positions: Record<string, { x: number; y: number }>
  ) => void;
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
  const { tx } = useI18n();
  const { isDark } = useTheme();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [quickAddNodeOpen, setQuickAddNodeOpen] = useState(false);
  const [newNodeName, setNewNodeName] = useState("");
  const [newNodeDisplay, setNewNodeDisplay] = useState("");
  const [quickAddEdgeOpen, setQuickAddEdgeOpen] = useState(false);
  const [newEdgeName, setNewEdgeName] = useState("SUPPLIES");
  const [newEdgeSrc, setNewEdgeSrc] = useState("");
  const [newEdgeDst, setNewEdgeDst] = useState("");
  const [layoutRunning, setLayoutRunning] = useState(false);

  const draggingNodeIdRef = useRef<string | null>(null);

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

  // Collision-free initial placement using full card dimensions (224x156 + 44px margin)
  const flowNodes: StudioFlowNode[] = useMemo(() => {
    const CARD_MIN_DX = 272;
    const CARD_MIN_DY = 188;
    const placed: Array<{ x: number; y: number }> = [];
    return nodeTypes.map((nt, idx) => {
      const st = issueStatsByNode[nt.name] ?? { count: 0, hasError: false };
      let x = Number.isFinite(nt.pos_x)
        ? nt.pos_x
        : 96 + (idx % 3) * 320;
      let y = Number.isFinite(nt.pos_y)
        ? nt.pos_y
        : 96 + Math.floor(idx / 3) * 212;
      let shiftStep = 0;
      while (
        placed.some(
          (p) =>
            Math.abs(p.x - x) < CARD_MIN_DX && Math.abs(p.y - y) < CARD_MIN_DY
        )
      ) {
        shiftStep += 1;
        if (shiftStep % 2 === 1) {
          y += CARD_MIN_DY;
        } else {
          x += CARD_MIN_DX;
          y -= CARD_MIN_DY;
        }
      }
      placed.push({ x, y });
      return {
        id: nt.name,
        type: "studioNode",
        position: { x, y },
        data: {
          spec: nt,
          selected: selectedNodeName === nt.name,
          issueCount: st.count,
          hasError: st.hasError,
        },
      };
    });
  }, [nodeTypes, selectedNodeName, issueStatsByNode]);

  const [liveNodes, setLiveNodes] = useState<StudioFlowNode[]>(flowNodes);

  useEffect(() => {
    setLiveNodes((prev) => {
      const prevMap = new Map(prev.map((n) => [n.id, n]));
      return flowNodes.map((nextNode) => {
        const existing = prevMap.get(nextNode.id);
        if (
          existing &&
          (existing.dragging || draggingNodeIdRef.current === nextNode.id)
        ) {
          return {
            ...nextNode,
            position: existing.position,
            measured: existing.measured,
            dragging: existing.dragging,
          };
        }
        if (existing?.measured) {
          return {
            ...nextNode,
            measured: existing.measured,
          };
        }
        return nextNode;
      });
    });
  }, [flowNodes]);

  const handleNodesChange: OnNodesChange<StudioFlowNode> = useCallback(
    (changes) => {
      setLiveNodes((nds) => applyNodeChanges(changes, nds));
    },
    []
  );

  const nodeNameSet = useMemo(
    () => new Set(nodeTypes.map((n) => n.name)),
    [nodeTypes]
  );

  const flowEdges: StudioFlowEdge[] = useMemo(() => {
    const activeStroke = isDark ? "#3b82f6" : "#2563eb";
    const idleStroke = isDark ? "#71717a" : "#94a3b8";
    return edgeTypes
      .map((et, idx) => ({ et, idx }))
      .filter(
        ({ et }) =>
          nodeNameSet.has(et.src_type_name) && nodeNameSet.has(et.dst_type_name)
      )
      .map(({ et, idx }) => ({
        id: `edge-${idx}-${et.src_type_name}-${et.name}-${et.dst_type_name}`,
        source: et.src_type_name,
        target: et.dst_type_name,
        type: "studioEdge",
        markerEnd: {
          type: MarkerType.ArrowClosed,
          color: selectedEdgeIndex === idx ? activeStroke : idleStroke,
        },
        data: {
          spec: et,
          edgeIndex: idx,
          selected: selectedEdgeIndex === idx,
          isDark,
          onSelectEdge: (edgeIdx: number) => {
            onSelectNode(null);
            onSelectEdge(edgeIdx);
          },
        },
      }));
  }, [
    edgeTypes,
    isDark,
    nodeNameSet,
    selectedEdgeIndex,
    onSelectEdge,
    onSelectNode,
  ]);

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!canEdit || !connection.source || !connection.target) return;
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
    [canEdit, onCreateEdgeType]
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
          "elk.layered.spacing.nodeNodeBetweenLayers": "180",
          "elk.spacing.nodeNode": "96",
          "elk.padding": "[top=64,left=48,bottom=48,right=48]",
        },
        children: nodeTypes.map((nt) => ({
          id: nt.name,
          width: 224,
          height: 152,
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
        if (
          child.id &&
          typeof child.x === "number" &&
          typeof child.y === "number"
        ) {
          positions[child.id] = {
            x: Math.round(child.x / 16) * 16,
            y: Math.round(child.y / 16) * 16,
          };
        }
      }
      onApplyLayoutPositions(positions);
    } catch {
      const fallback: Record<string, { x: number; y: number }> = {};
      nodeTypes.forEach((nt, idx) => {
        fallback[nt.name] = {
          x: 96 + (idx % 3) * 352,
          y: 96 + Math.floor(idx / 3) * 204,
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
      className="relative flex-1 min-h-[360px] w-full rounded-md border border-border bg-muted/15 overflow-hidden"
      data-testid="xbluestudio-canvas"
    >
      {/* Top Floating Canvas Toolbar (Deduplicated — counts live in PageHeader) */}
      <div className="absolute top-2.5 left-2.5 z-10 flex flex-wrap items-center gap-1.5 rounded-md border border-border bg-card/95 px-2 py-1.5 shadow-xs backdrop-blur-xs">
        {canEdit && (
          <>
            <Button
              variant="outline"
              size="sm"
              data-testid="canvas-add-node-btn"
              onClick={() => {
                setQuickAddNodeOpen((v) => !v);
                setQuickAddEdgeOpen(false);
              }}
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新增节点类型", "Add Node Type")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              data-testid="canvas-add-edge-btn"
              onClick={() => {
                setNewEdgeSrc(nodeTypes[0]?.name ?? "");
                setNewEdgeDst(nodeTypes[1]?.name ?? nodeTypes[0]?.name ?? "");
                setQuickAddEdgeOpen((v) => !v);
                setQuickAddNodeOpen(false);
              }}
            >
              <Link2 className="h-3.5 w-3.5" />
              {tx("新增关系连线", "Add Relation")}
            </Button>
          </>
        )}
        <Button
          variant="outline"
          size="sm"
          data-testid="canvas-elk-layout-btn"
          onClick={() => void runElkLayout()}
          disabled={layoutRunning || nodeTypes.length === 0}
        >
          <LayoutGrid className="h-3.5 w-3.5 text-xblue" />
          {layoutRunning
            ? tx("排布中…", "Arranging…")
            : tx("自动分层布局", "Auto Layout")}
        </Button>
      </div>

      {/* Quick Create Node Popover */}
      {quickAddNodeOpen && canEdit && (
        <div className="absolute top-14 left-2.5 z-20 w-80 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-medium">
            {tx("新建节点类型", "New Node Type")}
          </div>
          <div className="grid grid-cols-2 gap-1.5">
            <Input
              data-testid="quick-node-name-input"
              placeholder="Factory"
              value={newNodeName}
              onChange={(e) => setNewNodeName(e.target.value)}
              className="h-8 text-xs font-mono-tech"
            />
            <Input
              data-testid="quick-node-display-input"
              placeholder={tx("制造工厂", "Factory")}
              value={newNodeDisplay}
              onChange={(e) => setNewNodeDisplay(e.target.value)}
              className="h-8 text-xs"
            />
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setQuickAddNodeOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              data-testid="quick-node-submit-btn"
              size="sm"
              onClick={submitQuickNode}
            >
              {tx("创建", "Create")}
            </Button>
          </div>
        </div>
      )}

      {/* Quick Create Edge Popover */}
      {quickAddEdgeOpen && canEdit && (
        <div className="absolute top-14 left-2.5 z-20 w-96 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-medium">
            {tx("新建关系类型", "New Relation Type")}
          </div>
          <div className="grid grid-cols-3 gap-1.5">
            <Select
              value={newEdgeSrc}
              onChange={(e) => setNewEdgeSrc(e.target.value)}
              className="h-8 px-1.5 font-mono-tech"
            >
              <option value="">{tx("源类型", "Source")}</option>
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.name}
                </option>
              ))}
            </Select>
            <Input
              data-testid="quick-edge-name-input"
              value={newEdgeName}
              onChange={(e) => setNewEdgeName(e.target.value)}
              placeholder="FULFILLS"
              className="h-8 text-xs font-mono-tech"
            />
            <Select
              value={newEdgeDst}
              onChange={(e) => setNewEdgeDst(e.target.value)}
              className="h-8 px-1.5 font-mono-tech"
            >
              <option value="">{tx("目标类型", "Target")}</option>
              {nodeTypes.map((nt) => (
                <option key={nt.name} value={nt.name}>
                  {nt.name}
                </option>
              ))}
            </Select>
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setQuickAddEdgeOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              data-testid="quick-edge-submit-btn"
              size="sm"
              onClick={submitQuickEdge}
            >
              {tx("创建", "Create")}
            </Button>
          </div>
        </div>
      )}

      <ReactFlow
        nodes={liveNodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onNodesChange={handleNodesChange}
        nodesDraggable={canEdit}
        nodesConnectable={canEdit}
        snapToGrid
        snapGrid={[16, 16]}
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
        onNodeDragStart={(_event, node) => {
          draggingNodeIdRef.current = node.id;
        }}
        onNodeDragStop={(_event, node) => {
          draggingNodeIdRef.current = null;
          if (!canEdit) return;
          onUpdateNodePosition(
            node.id,
            Math.round(node.position.x),
            Math.round(node.position.y)
          );
        }}
        onConnect={handleConnect}
        fitView
        fitViewOptions={{ padding: 0.24, maxZoom: 0.95 }}
        proOptions={{ hideAttribution: true }}
      >
        <Background
          color={isDark ? "#27272a" : "#d4d4d8"}
          gap={16}
          size={1}
        />
        <Controls showInteractive={false} />
        {nodeTypes.length > 5 && (
          <MiniMap
            pannable
            zoomable
            style={{ width: 120, height: 80 }}
            nodeColor={(n) => {
              const spec = (n.data as StudioFlowNode["data"] | undefined)?.spec;
              return spec?.color || (isDark ? "#3b82f6" : "#2563eb");
            }}
            className="!bg-card !border !border-border !rounded"
          />
        )}
      </ReactFlow>

      {/* Non-anchored cross-subgraph edges indicator */}
      {edgeTypes.length > flowEdges.length && (
        <div className="absolute bottom-2.5 left-12 z-10 flex flex-wrap items-center gap-1 rounded border border-border bg-card/95 px-2.5 py-1 text-[11px]">
          <span className="text-muted-foreground">
            {tx("跨子图关系:", "Cross-subgraph:")}
          </span>
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
