import { useCallback, useMemo, useState } from "react";
import {
  Background,
  BaseEdge,
  Controls,
  EdgeLabelRenderer,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  getSmoothStepPath,
  type Connection,
  type Edge,
  type EdgeProps,
  type EdgeTypes,
  type Node,
  type NodeProps,
  type NodeTypes,
  type ReactFlowInstance,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs/lib/elk.bundled.js";
import {
  AlertCircle,
  CheckCircle2,
  Clock,
  LayoutGrid,
  Link2,
  Loader2,
  PauseCircle,
  Slash,
  Trash2,
  XCircle,
} from "lucide-react";
import type { FlowOperator, StepStatus } from "@/lib/api/enums";
import type {
  FlowEdgeDef,
  FlowIssue,
  FlowNodeDef,
} from "@/lib/api/axflow";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { renderOperatorIcon } from "./OperatorPalette";

export interface NodeStepRuntimeBadge {
  status: StepStatus;
  attempt: number;
}

export interface AxFlowCanvasNodeData extends Record<string, unknown> {
  node: FlowNodeDef;
  selected: boolean;
  issueCount: number;
  hasError: boolean;
  runtime: NodeStepRuntimeBadge | null;
}

export type AxFlowRfNode = Node<AxFlowCanvasNodeData, "axflowNode">;

export interface AxFlowCanvasEdgeData extends Record<string, unknown> {
  edgeDef: FlowEdgeDef;
  edgeIndex: number;
  selected: boolean;
  onSelectEdge: (index: number) => void;
}

export type AxFlowRfEdge = Edge<AxFlowCanvasEdgeData, "axflowEdge">;

function renderStepStatusBadge(
  nodeId: string,
  runtime: NodeStepRuntimeBadge
) {
  const { status, attempt } = runtime;
  const baseCls =
    "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border";

  switch (status) {
    case "SUCCEEDED":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
          )}
        >
          <CheckCircle2 className="h-2.5 w-2.5" />
          SUCCEEDED{attempt > 1 ? ` #${attempt}` : ""}
        </span>
      );
    case "WAITING_APPROVAL":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-amber-500/40 bg-amber-500/15 text-amber-700 dark:text-amber-300"
          )}
        >
          <PauseCircle className="h-2.5 w-2.5" />
          WAITING_APPROVAL
        </span>
      );
    case "RUNNING":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-xblue/40 bg-xblue-soft text-xblue"
          )}
        >
          <Loader2 className="h-2.5 w-2.5 animate-spin" />
          RUNNING #{attempt}
        </span>
      );
    case "FAILED":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-red-500/40 bg-red-500/10 text-red-700 dark:text-red-400"
          )}
        >
          <XCircle className="h-2.5 w-2.5" />
          FAILED #{attempt}
        </span>
      );
    case "SKIPPED":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-border bg-muted text-muted-foreground"
          )}
        >
          <Slash className="h-2.5 w-2.5" />
          SKIPPED
        </span>
      );
    case "PENDING":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(
            baseCls,
            "border-border bg-muted/60 text-muted-foreground"
          )}
        >
          <Clock className="h-2.5 w-2.5" />
          PENDING
        </span>
      );
  }
}

function AxFlowCanvasNodeCard({ data }: NodeProps<AxFlowRfNode>) {
  const { node, selected, issueCount, hasError, runtime } = data;
  const accentColor =
    runtime?.status === "FAILED"
      ? "#dc2626"
      : runtime?.status === "WAITING_APPROVAL"
        ? "#d97706"
        : runtime?.status === "SUCCEEDED"
          ? "#059669"
          : "#2563eb";

  return (
    <div
      data-testid={`flow-node-${node.id}`}
      className={cn(
        "w-56 rounded-md border bg-card text-card-foreground shadow-xs transition-shadow select-none overflow-hidden",
        selected
          ? "border-xblue ring-2 ring-xblue/30 shadow-md"
          : "border-border hover:border-foreground/40",
        hasError && "border-red-500"
      )}
    >
      <div className="h-1.5 w-full" style={{ backgroundColor: accentColor }} />
      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-foreground"
      />

      <div className="p-2.5 space-y-2">
        <div className="flex items-start justify-between gap-1.5">
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="inline-flex h-6 w-6 items-center justify-center rounded border border-border bg-muted text-xblue shrink-0">
              {renderOperatorIcon(node.kind)}
            </span>
            <div className="min-w-0">
              <div className="truncate text-xs font-semibold leading-tight">
                {node.label || node.id}
              </div>
              <div className="truncate font-mono-tech text-[10px] text-muted-foreground">
                {node.id}
              </div>
            </div>
          </div>

          {issueCount > 0 && (
            <span
              className={cn(
                "inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 font-mono-tech text-[10px] font-semibold shrink-0",
                hasError
                  ? "bg-red-500/15 text-red-600 dark:text-red-400"
                  : "bg-amber-500/15 text-amber-600 dark:text-amber-400"
              )}
            >
              <AlertCircle className="h-2.5 w-2.5" />
              {issueCount}
            </span>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-1">
          <Badge variant="xblue" className="text-[10px]">
            {node.kind}
          </Badge>
          {node.join !== "ALL_SUCCESS" && (
            <Badge variant="outline" className="text-[10px]">
              {node.join}
            </Badge>
          )}
          {node.on_failure === "CONTINUE" && (
            <Badge variant="neutral" className="text-[10px]">
              CONTINUE
            </Badge>
          )}
          <Badge variant="outline" className="text-[10px]">
            重试×{node.retry.max_attempts}
          </Badge>
        </div>

        {runtime && (
          <div className="pt-1 border-t border-border flex items-center justify-between">
            {renderStepStatusBadge(node.id, runtime)}
          </div>
        )}
      </div>

      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-xblue"
      />
    </div>
  );
}

function AxFlowConditionEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  data,
}: EdgeProps<AxFlowRfEdge>) {
  const [edgePath, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    borderRadius: 8,
  });

  const edgeDef = data?.edgeDef;
  const selected = Boolean(data?.selected);
  const whenExpr = edgeDef?.when?.trim() ?? "";

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: selected ? "#2563eb" : whenExpr ? "#1d4ed8" : "#71717a",
          strokeWidth: selected ? 2.4 : 1.6,
          strokeDasharray: whenExpr ? "5 3" : undefined,
        }}
      />
      {edgeDef && (
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
              data-testid={`flow-edge-${edgeDef.from}-${edgeDef.to}`}
              onClick={(e) => {
                e.stopPropagation();
                data?.onSelectEdge(data.edgeIndex);
              }}
              className={cn(
                "inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs transition-colors cursor-pointer",
                selected
                  ? "border-xblue bg-xblue text-white"
                  : whenExpr
                    ? "border-xblue/40 bg-xblue-soft text-xblue hover:border-xblue"
                    : "border-border bg-card/90 text-muted-foreground hover:text-foreground hover:border-xblue"
              )}
              title="点击编辑分支条件 (when)"
            >
              {whenExpr ? `when: ${whenExpr}` : "无条件"}
            </button>
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

const NODE_TYPES: NodeTypes = {
  axflowNode: AxFlowCanvasNodeCard,
};

const EDGE_TYPES: EdgeTypes = {
  axflowEdge: AxFlowConditionEdge,
};

export interface FlowCanvasProps {
  nodes: FlowNodeDef[];
  edges: FlowEdgeDef[];
  issues: FlowIssue[];
  stepStatusByNodeId: Record<string, NodeStepRuntimeBadge>;
  selectedNodeId: string | null;
  selectedEdgeIndex: number | null;
  onSelectNode: (nodeId: string | null) => void;
  onSelectEdge: (edgeIndex: number | null) => void;
  onUpdateNodePos: (nodeId: string, pos: { x: number; y: number }) => void;
  onApplyLayout: (positions: Record<string, { x: number; y: number }>) => void;
  onConnectNodes: (from: string, to: string, when?: string | null) => void;
  onUpdateEdge: (edgeIndex: number, updated: FlowEdgeDef) => void;
  onDeleteEdge: (edgeIndex: number) => void;
  onDropOperator: (kind: FlowOperator, pos: { x: number; y: number }) => void;
}

export function FlowCanvas({
  nodes,
  edges,
  issues,
  stepStatusByNodeId,
  selectedNodeId,
  selectedEdgeIndex,
  onSelectNode,
  onSelectEdge,
  onUpdateNodePos,
  onApplyLayout,
  onConnectNodes,
  onUpdateEdge,
  onDeleteEdge,
  onDropOperator,
}: FlowCanvasProps) {
  const [rfInstance, setRfInstance] = useState<ReactFlowInstance<
    AxFlowRfNode,
    AxFlowRfEdge
  > | null>(null);
  const [quickConnectOpen, setQuickConnectOpen] = useState(false);
  const [connectFrom, setConnectFrom] = useState("");
  const [connectTo, setConnectTo] = useState("");
  const [connectWhen, setConnectWhen] = useState("");
  const [layoutRunning, setLayoutRunning] = useState(false);

  const issueStatsByNode = useMemo(() => {
    const stats: Record<string, { count: number; hasError: boolean }> = {};
    for (const issue of issues) {
      if (issue.node_id) {
        const prev = stats[issue.node_id] ?? { count: 0, hasError: false };
        stats[issue.node_id] = {
          count: prev.count + 1,
          hasError: prev.hasError || issue.severity === "ERROR",
        };
      }
    }
    return stats;
  }, [issues]);

  const rfNodes: AxFlowRfNode[] = useMemo(
    () =>
      nodes.map((node, idx) => {
        const st = issueStatsByNode[node.id] ?? { count: 0, hasError: false };
        return {
          id: node.id,
          type: "axflowNode",
          position: {
            x: Number.isFinite(node.pos.x) ? node.pos.x : 80 + idx * 250,
            y: Number.isFinite(node.pos.y) ? node.pos.y : 140,
          },
          data: {
            node,
            selected: selectedNodeId === node.id,
            issueCount: st.count,
            hasError: st.hasError,
            runtime: stepStatusByNodeId[node.id] ?? null,
          },
        };
      }),
    [nodes, selectedNodeId, issueStatsByNode, stepStatusByNodeId]
  );

  const nodeIdSet = useMemo(() => new Set(nodes.map((n) => n.id)), [nodes]);

  const rfEdges: AxFlowRfEdge[] = useMemo(
    () =>
      edges
        .map((edgeDef, idx) => ({ edgeDef, idx }))
        .filter(
          ({ edgeDef }) =>
            nodeIdSet.has(edgeDef.from) && nodeIdSet.has(edgeDef.to)
        )
        .map(({ edgeDef, idx }) => ({
          id: `axflow-edge-${idx}-${edgeDef.from}-${edgeDef.to}`,
          source: edgeDef.from,
          target: edgeDef.to,
          type: "axflowEdge",
          markerEnd: {
            type: MarkerType.ArrowClosed,
            color: selectedEdgeIndex === idx ? "#2563eb" : "#71717a",
          },
          data: {
            edgeDef,
            edgeIndex: idx,
            selected: selectedEdgeIndex === idx,
            onSelectEdge: (edgeIdx: number) => {
              onSelectNode(null);
              onSelectEdge(edgeIdx);
            },
          },
        })),
    [edges, nodeIdSet, selectedEdgeIndex, onSelectEdge, onSelectNode]
  );

  const handleConnect = useCallback(
    (conn: Connection) => {
      if (!conn.source || !conn.target || conn.source === conn.target) return;
      onConnectNodes(conn.source, conn.target, null);
    },
    [onConnectNodes]
  );

  const runElkLayout = async () => {
    if (nodes.length === 0 || layoutRunning) return;
    setLayoutRunning(true);
    try {
      const elk = new ELK();
      const laidOut = await elk.layout({
        id: "axflow-root",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "RIGHT",
          "elk.layered.spacing.nodeNodeBetweenLayers": "96",
          "elk.spacing.nodeNode": "64",
          "elk.padding": "[top=56,left=40,bottom=40,right=40]",
        },
        children: nodes.map((n) => ({
          id: n.id,
          width: 224,
          height: 120,
        })),
        edges: edges
          .filter((e) => nodeIdSet.has(e.from) && nodeIdSet.has(e.to))
          .map((e, idx) => ({
            id: `elk-edge-${idx}`,
            sources: [e.from],
            targets: [e.to],
          })),
      });

      const positions: Record<string, { x: number; y: number }> = {};
      for (const child of laidOut.children ?? []) {
        if (
          child.id &&
          typeof child.x === "number" &&
          typeof child.y === "number"
        ) {
          positions[child.id] = {
            x: Math.round(child.x),
            y: Math.round(child.y),
          };
        }
      }
      onApplyLayout(positions);
    } catch {
      const fallback: Record<string, { x: number; y: number }> = {};
      nodes.forEach((n, idx) => {
        fallback[n.id] = {
          x: 60 + (idx % 4) * 260,
          y: 80 + Math.floor(idx / 4) * 170,
        };
      });
      onApplyLayout(fallback);
    } finally {
      setLayoutRunning(false);
    }
  };

  const selectedEdge =
    selectedEdgeIndex !== null ? edges[selectedEdgeIndex] ?? null : null;

  return (
    <div
      className="relative h-[540px] w-full rounded-md border border-border bg-muted/20 overflow-hidden"
      data-testid="axflow-canvas"
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }}
      onDrop={(e) => {
        e.preventDefault();
        const kind = e.dataTransfer.getData("application/axflow-operator");
        if (!kind) return;
        const pos = rfInstance
          ? rfInstance.screenToFlowPosition({ x: e.clientX, y: e.clientY })
          : { x: 140 + nodes.length * 60, y: 140 };
        onDropOperator(kind as FlowOperator, {
          x: Math.round(pos.x),
          y: Math.round(pos.y),
        });
      }}
    >
      {/* Top Canvas Toolbar */}
      <div className="absolute top-2.5 left-2.5 right-2.5 z-10 flex flex-wrap items-center justify-between gap-2 pointer-events-none">
        <div className="flex flex-wrap items-center gap-1.5 pointer-events-auto rounded-md border border-border bg-card/95 px-2 py-1.5 shadow-xs">
          <Button
            type="button"
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="axflow-quick-connect-btn"
            disabled={nodes.length < 2}
            onClick={() => {
              setConnectFrom(nodes[0]?.id ?? "");
              setConnectTo(nodes[1]?.id ?? nodes[0]?.id ?? "");
              setConnectWhen("");
              setQuickConnectOpen((v) => !v);
            }}
          >
            <Link2 className="h-3.5 w-3.5" />
            添加依赖边
          </Button>
          <Button
            type="button"
            variant="outline"
            className="h-7 px-2.5 py-1 text-xs"
            data-testid="axflow-elk-layout-btn"
            disabled={nodes.length === 0 || layoutRunning}
            onClick={() => void runElkLayout()}
          >
            <LayoutGrid className="h-3.5 w-3.5 text-xblue" />
            {layoutRunning ? "计算中..." : "自动拓扑排布"}
          </Button>
        </div>

        <div className="pointer-events-auto rounded-md border border-border bg-card/95 px-2.5 py-1 text-[11px] font-mono-tech text-muted-foreground">
          节点 {nodes.length} · 边 {edges.length}
        </div>
      </div>

      {/* Quick Edge Creation Popover */}
      {quickConnectOpen && (
        <div className="absolute top-13 left-2.5 z-20 w-80 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-semibold">新建有向依赖边</div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                上游节点 (from)
              </label>
              <select
                value={connectFrom}
                onChange={(e) => setConnectFrom(e.target.value)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.id} ({n.label})
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                下游节点 (to)
              </label>
              <select
                value={connectTo}
                onChange={(e) => setConnectTo(e.target.value)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.id} ({n.label})
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              分支守卫表达式 when (可选)
            </label>
            <Input
              value={connectWhen}
              onChange={(e) => setConnectWhen(e.target.value)}
              placeholder="例如: n > 0"
              className="h-8 font-mono-tech text-xs"
            />
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              type="button"
              variant="ghost"
              className="h-7 px-2.5"
              onClick={() => setQuickConnectOpen(false)}
            >
              取消
            </Button>
            <Button
              type="button"
              className="h-7 px-2.5"
              disabled={!connectFrom || !connectTo || connectFrom === connectTo}
              onClick={() => {
                onConnectNodes(
                  connectFrom,
                  connectTo,
                  connectWhen.trim() || null
                );
                setQuickConnectOpen(false);
              }}
            >
              确认连线
            </Button>
          </div>
        </div>
      )}

      {/* Selected Edge Inline `when` Guard Bar */}
      {selectedEdge && selectedEdgeIndex !== null && (
        <div
          data-testid="axflow-edge-inline-editor"
          className="absolute bottom-3 left-3 right-36 z-20 flex flex-wrap items-center gap-2 rounded-md border border-xblue/40 bg-card/95 px-3 py-2 shadow-md text-xs"
        >
          <span className="font-mono-tech font-semibold text-xblue">
            {selectedEdge.from} → {selectedEdge.to}
          </span>
          <span className="text-muted-foreground">when:</span>
          <Input
            data-testid="canvas-edge-when-input"
            value={selectedEdge.when ?? ""}
            onChange={(e) =>
              onUpdateEdge(selectedEdgeIndex, {
                ...selectedEdge,
                when: e.target.value.length > 0 ? e.target.value : null,
              })
            }
            placeholder="留空为无条件激活；如 n > 0"
            className="h-7 flex-1 min-w-36 font-mono-tech text-xs"
          />
          <Button
            type="button"
            variant="outline"
            className="h-7 px-2 text-[11px]"
            onClick={() => onDeleteEdge(selectedEdgeIndex)}
          >
            <Trash2 className="h-3 w-3" />
            删除边
          </Button>
        </div>
      )}

      <ReactFlow
        nodes={rfNodes}
        edges={rfEdges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onInit={setRfInstance}
        onConnect={handleConnect}
        onNodeClick={(_, node) => {
          onSelectEdge(null);
          onSelectNode(node.id);
        }}
        onNodeDragStop={(_, node) => {
          onUpdateNodePos(node.id, {
            x: Math.round(node.position.x),
            y: Math.round(node.position.y),
          });
        }}
        onPaneClick={() => {
          onSelectNode(null);
          onSelectEdge(null);
        }}
        fitView
        fitViewOptions={{ padding: 0.22 }}
        proOptions={{ hideAttribution: true }}
      >
        <Background gap={18} size={1} />
        <Controls showInteractive={false} />
        <MiniMap
          pannable
          zoomable
          className="!bg-card !border !border-border rounded"
        />
      </ReactFlow>
    </div>
  );
}
