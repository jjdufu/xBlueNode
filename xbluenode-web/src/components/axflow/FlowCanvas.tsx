import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  applyNodeChanges,
  getSmoothStepPath,
  type Connection,
  type Edge,
  type EdgeProps,
  type EdgeTypes,
  type Node,
  type NodeProps,
  type NodeTypes,
  type OnNodesChange,
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
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import type { FlowOperator, StepStatus } from "@/lib/api/enums";
import type {
  FlowEdgeDef,
  FlowIssue,
  FlowNodeDef,
} from "@/lib/api/axflow";
import { Badge, Button, Input, Select } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  OPERATOR_I18N,
  clearActiveDraggedOperator,
  getActiveDraggedOperator,
  renderOperatorIcon,
} from "./OperatorPalette";

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
  isDark: boolean;
  unconditionalLabel: string;
  onSelectEdge: (index: number) => void;
}

export type AxFlowRfEdge = Edge<AxFlowCanvasEdgeData, "axflowEdge">;

function renderStepStatusBadge(
  nodeId: string,
  runtime: NodeStepRuntimeBadge,
  tEnum: (code: string | null | undefined) => string
) {
  const { status, attempt } = runtime;
  const baseCls =
    "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border tabular-nums";
  const label = tEnum(status);

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
          {label}
          {attempt > 1 ? ` #${attempt}` : ""}
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
          {label}
        </span>
      );
    case "RUNNING":
      return (
        <span
          data-testid={`canvas-node-status-${nodeId}`}
          className={cn(baseCls, "border-xblue/40 bg-xblue-soft text-xblue")}
        >
          <Loader2 className="h-2.5 w-2.5 animate-spin" />
          {label} #{attempt}
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
          {label} #{attempt}
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
          {label}
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
          {label}
        </span>
      );
  }
}

function AxFlowCanvasNodeCard({ data }: NodeProps<AxFlowRfNode>) {
  const { isZh, tx, tEnum } = useI18n();
  const { node, selected, issueCount, hasError, runtime } = data;
  const opMeta = OPERATOR_I18N[node.kind];
  const opLabel = opMeta
    ? isZh
      ? opMeta.zh
      : opMeta.en
    : tEnum(node.kind);
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
          <Badge variant="xblue" className="text-[10px]" title={node.kind}>
            {opLabel}
          </Badge>
          {node.join !== "ALL_SUCCESS" && (
            <Badge variant="outline" className="text-[10px]" title={node.join}>
              {tEnum(node.join)}
            </Badge>
          )}
          {node.on_failure === "CONTINUE" && (
            <Badge variant="neutral" className="text-[10px]">
              {tx("继续执行", "Continue")}
            </Badge>
          )}
          <Badge variant="outline" className="text-[10px] tabular-nums">
            {tx("重试", "Retry")}×{node.retry.max_attempts}
          </Badge>
        </div>

        {runtime && (
          <div className="pt-1 border-t border-border flex items-center justify-between">
            {renderStepStatusBadge(node.id, runtime, tEnum)}
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
  const isDark = Boolean(data?.isDark);
  const whenExpr = edgeDef?.when?.trim() ?? "";

  const strokeColor = selected
    ? isDark
      ? "#3b82f6"
      : "#2563eb"
    : whenExpr
      ? isDark
        ? "#60a5fa"
        : "#1d4ed8"
      : isDark
        ? "#a1a1aa"
        : "#71717a";

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: strokeColor,
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
            >
              {whenExpr
                ? `when: ${whenExpr}`
                : (data?.unconditionalLabel ?? "Always")}
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
  const { isZh, tx } = useI18n();
  const { isDark } = useTheme();
  const { can } = usePrincipal();
  const canEdit = can("FLOW_EDIT");

  const canvasContainerRef = useRef<HTMLDivElement | null>(null);
  const draggingNodeIdRef = useRef<string | null>(null);

  const [rfInstance, setRfInstance] = useState<ReactFlowInstance<
    AxFlowRfNode,
    AxFlowRfEdge
  > | null>(null);
  const [dragGhost, setDragGhost] = useState<{
    kind: FlowOperator;
    label: string;
    flowX: number;
    flowY: number;
    screenX: number;
    screenY: number;
  } | null>(null);

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
            x: Number.isFinite(node.pos.x) ? node.pos.x : 80 + idx * 288,
            y: Number.isFinite(node.pos.y) ? node.pos.y : 144,
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

  const [liveNodes, setLiveNodes] = useState<AxFlowRfNode[]>(rfNodes);

  useEffect(() => {
    setLiveNodes((prev) => {
      const prevMap = new Map(prev.map((n) => [n.id, n]));
      return rfNodes.map((nextNode) => {
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
  }, [rfNodes]);

  const handleNodesChange: OnNodesChange<AxFlowRfNode> = useCallback(
    (changes) => {
      setLiveNodes((nds) => applyNodeChanges(changes, nds));
    },
    []
  );

  const nodeIdSet = useMemo(() => new Set(nodes.map((n) => n.id)), [nodes]);
  const unconditionalLabel = tx("无条件", "Always");

  const rfEdges: AxFlowRfEdge[] = useMemo(() => {
    const activeStroke = isDark ? "#3b82f6" : "#2563eb";
    const idleStroke = isDark ? "#a1a1aa" : "#71717a";
    return edges
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
          color: selectedEdgeIndex === idx ? activeStroke : idleStroke,
        },
        data: {
          edgeDef,
          edgeIndex: idx,
          selected: selectedEdgeIndex === idx,
          isDark,
          unconditionalLabel,
          onSelectEdge: (edgeIdx: number) => {
            onSelectNode(null);
            onSelectEdge(edgeIdx);
          },
        },
      }));
  }, [
    edges,
    isDark,
    nodeIdSet,
    selectedEdgeIndex,
    unconditionalLabel,
    onSelectEdge,
    onSelectNode,
  ]);

  const handleConnect = useCallback(
    (conn: Connection) => {
      if (
        !canEdit ||
        !conn.source ||
        !conn.target ||
        conn.source === conn.target
      ) {
        return;
      }
      onConnectNodes(conn.source, conn.target, null);
    },
    [canEdit, onConnectNodes]
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
          "elk.layered.spacing.nodeNodeBetweenLayers": "136",
          "elk.spacing.nodeNode": "72",
          "elk.padding": "[top=56,left=48,bottom=48,right=48]",
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
            x: Math.round(child.x / 16) * 16,
            y: Math.round(child.y / 16) * 16,
          };
        }
      }
      onApplyLayout(positions);
    } catch {
      const fallback: Record<string, { x: number; y: number }> = {};
      nodes.forEach((n, idx) => {
        fallback[n.id] = {
          x: 64 + (idx % 4) * 288,
          y: 80 + Math.floor(idx / 4) * 176,
        };
      });
      onApplyLayout(fallback);
    } finally {
      setLayoutRunning(false);
    }
  };

  const selectedEdge =
    selectedEdgeIndex !== null ? (edges[selectedEdgeIndex] ?? null) : null;

  return (
    <div
      ref={canvasContainerRef}
      className="relative flex-1 min-h-[340px] h-full w-full rounded-md border border-border bg-muted/15 overflow-hidden"
      data-testid="axflow-canvas"
      onDragOver={(e) => {
        if (!canEdit) return;
        const activeMeta = getActiveDraggedOperator();
        const hasOpType =
          activeMeta !== null ||
          Array.from(e.dataTransfer.types).includes(
            "application/axflow-operator"
          );
        if (!hasOpType) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";

        const rect = canvasContainerRef.current?.getBoundingClientRect();
        const screenX = rect ? e.clientX - rect.left : e.clientX;
        const screenY = rect ? e.clientY - rect.top : e.clientY;
        const rawFlow = rfInstance
          ? rfInstance.screenToFlowPosition({ x: e.clientX, y: e.clientY })
          : { x: screenX, y: screenY };
        const flowX = Math.round((rawFlow.x - 104) / 16) * 16;
        const flowY = Math.round((rawFlow.y - 48) / 16) * 16;

        const kind: FlowOperator = activeMeta?.kind ?? "SQL_TRANSFORM";
        const i18nMeta = OPERATOR_I18N[kind];
        const label =
          activeMeta?.label ??
          (i18nMeta ? (isZh ? i18nMeta.zh : i18nMeta.en) : kind);

        setDragGhost((prev) => {
          if (
            prev &&
            prev.kind === kind &&
            prev.screenX === screenX &&
            prev.screenY === screenY &&
            prev.flowX === flowX &&
            prev.flowY === flowY
          ) {
            return prev;
          }
          return { kind, label, flowX, flowY, screenX, screenY };
        });
      }}
      onDragLeave={(e) => {
        const nextTarget = e.relatedTarget as globalThis.Node | null;
        if (!nextTarget || !e.currentTarget.contains(nextTarget)) {
          setDragGhost(null);
        }
      }}
      onDrop={(e) => {
        e.preventDefault();
        const activeMeta = getActiveDraggedOperator();
        const rawKind =
          e.dataTransfer.getData("application/axflow-operator") ||
          activeMeta?.kind ||
          "";
        setDragGhost(null);
        clearActiveDraggedOperator();
        if (!canEdit || !rawKind) return;

        const rawPos = rfInstance
          ? rfInstance.screenToFlowPosition({ x: e.clientX, y: e.clientY })
          : { x: 144 + nodes.length * 64, y: 144 };
        const flowX = Math.round((rawPos.x - 104) / 16) * 16;
        const flowY = Math.round((rawPos.y - 48) / 16) * 16;

        onDropOperator(rawKind as FlowOperator, {
          x: flowX,
          y: flowY,
        });
      }}
    >
      {/* Top Canvas Toolbar */}
      <div className="absolute top-2.5 left-2.5 right-2.5 z-10 flex flex-wrap items-center justify-between gap-2 pointer-events-none">
        <div className="flex flex-wrap items-center gap-1.5 pointer-events-auto rounded-md border border-border bg-card/95 px-2 py-1.5 shadow-xs">
          {canEdit && (
            <Button
              type="button"
              variant="outline"
              size="sm"
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
              {tx("添加依赖边", "Add Edge")}
            </Button>
          )}
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="axflow-elk-layout-btn"
            disabled={nodes.length === 0 || layoutRunning}
            onClick={() => void runElkLayout()}
          >
            <LayoutGrid className="h-3.5 w-3.5 text-xblue" />
            {layoutRunning
              ? tx("排布中…", "Arranging…")
              : tx("自动拓扑排布", "Auto Layout")}
          </Button>
        </div>

        <div className="pointer-events-auto rounded-md border border-border bg-card/95 px-2.5 py-1 text-[11px] font-mono-tech text-muted-foreground">
          {tx("节点", "Nodes")} {nodes.length} · {tx("边", "Edges")}{" "}
          {edges.length}
        </div>
      </div>

      {/* Dashed Ghost Drop Preview Box */}
      {dragGhost && (
        <div
          style={{
            left: `${dragGhost.screenX}px`,
            top: `${dragGhost.screenY}px`,
          }}
          data-testid="axflow-drop-ghost"
          className="pointer-events-none absolute z-20 w-52 h-24 -translate-x-1/2 -translate-y-1/2 rounded-md border-2 border-dashed border-xblue bg-xblue-soft/35 flex flex-col items-center justify-center gap-1 text-xs font-medium text-xblue shadow-sm"
        >
          <div className="flex items-center gap-1.5 px-2 max-w-full">
            {renderOperatorIcon(dragGhost.kind, "h-4 w-4 shrink-0")}
            <span className="truncate">{dragGhost.label}</span>
          </div>
          <span className="font-mono-tech text-[10px] opacity-85">
            ({dragGhost.flowX}, {dragGhost.flowY})
          </span>
        </div>
      )}

      {/* Quick Edge Creation Popover */}
      {quickConnectOpen && canEdit && (
        <div className="absolute top-13 left-2.5 z-20 w-80 rounded-md border border-border bg-card p-3 shadow-md space-y-2 text-xs">
          <div className="font-semibold">
            {tx("新建依赖边", "New Dependency Edge")}
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("上游节点", "Upstream")}
              </label>
              <Select
                value={connectFrom}
                onChange={(e) => setConnectFrom(e.target.value)}
                className="h-8 w-full font-mono-tech"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.id} ({n.label})
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("下游节点", "Downstream")}
              </label>
              <Select
                value={connectTo}
                onChange={(e) => setConnectTo(e.target.value)}
                className="h-8 w-full font-mono-tech"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.id} ({n.label})
                  </option>
                ))}
              </Select>
            </div>
          </div>
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("分支守卫条件", "Guard Condition")}
            </label>
            <Input
              value={connectWhen}
              onChange={(e) => setConnectWhen(e.target.value)}
              placeholder="n > 0"
              className="h-8 font-mono-tech text-xs"
            />
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setQuickConnectOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              size="sm"
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
              {tx("添加", "Add")}
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
          <span className="text-muted-foreground font-mono-tech">when:</span>
          <Input
            data-testid="canvas-edge-when-input"
            value={selectedEdge.when ?? ""}
            disabled={!canEdit}
            onChange={(e) =>
              onUpdateEdge(selectedEdgeIndex, {
                ...selectedEdge,
                when: e.target.value.length > 0 ? e.target.value : null,
              })
            }
            placeholder="n > 0"
            className="h-7 flex-1 min-w-36 font-mono-tech text-xs"
          />
          {canEdit && (
            <Button
              type="button"
              variant="danger"
              size="sm"
              onClick={() => onDeleteEdge(selectedEdgeIndex)}
            >
              <Trash2 className="h-3 w-3" />
              {tx("删除边", "Delete")}
            </Button>
          )}
        </div>
      )}

      <ReactFlow
        nodes={liveNodes}
        edges={rfEdges}
        nodeTypes={NODE_TYPES}
        edgeTypes={EDGE_TYPES}
        onInit={setRfInstance}
        onNodesChange={handleNodesChange}
        nodesDraggable={canEdit}
        nodesConnectable={canEdit}
        snapToGrid
        snapGrid={[16, 16]}
        onConnect={handleConnect}
        onNodeClick={(_, node) => {
          onSelectEdge(null);
          onSelectNode(node.id);
        }}
        onEdgeClick={(_, edge) => {
          if (typeof edge.data?.edgeIndex === "number") {
            onSelectNode(null);
            onSelectEdge(edge.data.edgeIndex);
          }
        }}
        onNodeDragStart={(_, node) => {
          draggingNodeIdRef.current = node.id;
        }}
        onNodeDragStop={(_, node) => {
          draggingNodeIdRef.current = null;
          if (!canEdit) return;
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
        fitViewOptions={{ padding: 0.28, maxZoom: 1.0 }}
        proOptions={{ hideAttribution: true }}
      >
        <Background
          color={isDark ? "#27272a" : "#d4d4d8"}
          gap={16}
          size={1}
        />
        <Controls showInteractive={false} />
        {nodes.length > 6 && (
          <MiniMap
            pannable
            zoomable
            style={{ width: 120, height: 80 }}
            nodeColor={(n) => {
              const rt = (n.data as AxFlowRfNode["data"] | undefined)?.runtime;
              if (rt?.status === "FAILED") return "#dc2626";
              if (rt?.status === "WAITING_APPROVAL") return "#d97706";
              if (rt?.status === "SUCCEEDED") return "#059669";
              return isDark ? "#3b82f6" : "#2563eb";
            }}
            className="!bg-card !border !border-border rounded"
          />
        )}
      </ReactFlow>
    </div>
  );
}
