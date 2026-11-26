import { useState } from "react";
import {
  BaseEdge,
  EdgeLabelRenderer,
  getSmoothStepPath,
  type Edge,
  type EdgeProps,
} from "@xyflow/react";
import { Plus } from "lucide-react";
import type { FlowEdgeDef } from "@/lib/api/axflow";
import { cn } from "@/lib/utils";

export interface AxFlowCanvasEdgeData extends Record<string, unknown> {
  edgeDef: FlowEdgeDef;
  edgeIndex: number;
  selected: boolean;
  highlighted?: boolean;
  dimmed?: boolean;
  isDark: boolean;
  canEdit: boolean;
  isRunningFlow: boolean;
  isDropTarget: boolean;
  dropInsertLabel: string;
  unconditionalLabel: string;
  insertAriaLabel: string;
  onSelectEdge: (index: number) => void;
  onOpenInsertPopover: (
    edgeIndex: number,
    anchor: { clientX: number; clientY: number; flowX: number; flowY: number }
  ) => void;
}

export type AxFlowRfEdge = Edge<AxFlowCanvasEdgeData, "axflowEdge">;

/**
 * Interactive `AxFlow` edge with midpoint `6px -> 20px +` operator insertion trigger,
 * `< 18px` drag-splice highlight, and `RUNNING` pulse animation.
 */
export function InsertableEdge({
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
  const [hovered, setHovered] = useState(false);

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
  const highlighted = Boolean(data?.highlighted);
  const dimmed = Boolean(data?.dimmed) && !selected && !highlighted;
  const isDark = Boolean(data?.isDark);
  const canEdit = Boolean(data?.canEdit);
  const isRunningFlow = Boolean(data?.isRunningFlow);
  const isDropTarget = Boolean(data?.isDropTarget);
  const whenExpr = edgeDef?.when?.trim() ?? "";

  const strokeColor = isDropTarget
    ? "#2563eb"
    : isRunningFlow || highlighted
      ? "#3b82f6"
      : selected
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
        className={isRunningFlow ? "axflow-running-edge" : undefined}
        style={{
          stroke: strokeColor,
          strokeWidth: isDropTarget
            ? 3
            : selected || isRunningFlow || highlighted
              ? 2.4
              : 1.6,
          opacity: dimmed ? 0.16 : 1,
          transition: "opacity 150ms ease, stroke-width 150ms ease",
          strokeDasharray: isDropTarget
            ? "6 4"
            : isRunningFlow
              ? "6 4"
              : whenExpr
                ? "5 3"
                : undefined,
        }}
      />

      {/* Invisible 24px hover corridor along edge path */}
      <path
        d={edgePath}
        fill="none"
        stroke="transparent"
        strokeWidth={24}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      />

      {edgeDef && (
        <EdgeLabelRenderer>
          <div
            style={{
              position: "absolute",
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
              pointerEvents: "all",
            }}
            onMouseEnter={() => setHovered(true)}
            onMouseLeave={() => setHovered(false)}
            className="nodrag nopan flex flex-col items-center gap-1"
          >
            {isDropTarget ? (
              <span
                data-testid={`flow-edge-drop-badge-${edgeDef.from}-${edgeDef.to}`}
                className="rounded-full border border-xblue bg-xblue px-2 py-0.5 text-[10px] font-medium text-white shadow-sm whitespace-nowrap"
              >
                {data?.dropInsertLabel ?? "Drop to insert"}
              </span>
            ) : (
              <div className="inline-flex items-center gap-1">
                <button
                  type="button"
                  data-testid={`flow-edge-${edgeDef.from}-${edgeDef.to}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    data?.onSelectEdge(data.edgeIndex);
                  }}
                  className={cn(
                    "inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs transition-colors cursor-pointer whitespace-nowrap",
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

                {canEdit && (
                  <button
                    type="button"
                    data-testid={`flow-edge-insert-btn-${edgeDef.from}-${edgeDef.to}`}
                    aria-label={data?.insertAriaLabel ?? "Insert operator"}
                    title={data?.insertAriaLabel ?? "Insert operator"}
                    onClick={(e) => {
                      e.stopPropagation();
                      data?.onOpenInsertPopover(data.edgeIndex, {
                        clientX: e.clientX,
                        clientY: e.clientY,
                        flowX: labelX,
                        flowY: labelY,
                      });
                    }}
                    className="group inline-flex h-6 w-6 items-center justify-center rounded-full cursor-pointer"
                  >
                    <span
                      className={cn(
                        "inline-flex items-center justify-center rounded-full transition-all duration-150",
                        hovered
                          ? "h-5 w-5 bg-xblue text-white shadow-sm"
                          : "h-1.5 w-1.5 bg-xblue/80 group-hover:h-5 group-hover:w-5 group-hover:bg-xblue group-hover:text-white"
                      )}
                    >
                      <Plus
                        className={cn(
                          "h-3 w-3 transition-opacity duration-150",
                          hovered
                            ? "opacity-100"
                            : "opacity-0 group-hover:opacity-100"
                        )}
                      />
                    </span>
                  </button>
                )}
              </div>
            )}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}
