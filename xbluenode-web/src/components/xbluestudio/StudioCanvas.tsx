import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BaseEdge,
  ControlButton,
  Controls,
  EdgeLabelRenderer,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  SelectionMode,
  applyNodeChanges,
  getSmoothStepPath,
  useNodesInitialized,
  useReactFlow,
  type Connection,
  type Edge,
  type EdgeProps,
  type EdgeTypes,
  type NodeTypes,
  type OnNodeDrag,
  type OnNodesChange,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  Blocks,
  GitBranch,
  GitMerge,
  Layers,
  LayoutGrid,
  Link2,
  LocateFixed,
  Maximize2,
  Minimize2,
  MousePointerSquareDashed,
  Plus,
  Redo2,
  Undo2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import type { Cardinality, StudioCanvasViewMode } from "@/lib/api/enums";
import type {
  EdgeTypeSpec,
  MappingSpec,
  NodeConsumptionCounts,
  NodeTypeSpec,
  SchemaDoc,
  ValidationIssue,
} from "@/lib/api/xblue";
import {
  HelperLinesOverlay,
} from "@/components/canvas/HelperLinesOverlay";
import {
  SelectionAlignToolbar,
} from "@/components/canvas/SelectionAlignToolbar";
import {
  computeAlignmentGuides,
  computeSelectionAlignment,
  type AlignAction,
  type HorizontalGuideLine,
  type MeasuredBox,
  type VerticalGuideLine,
} from "@/components/canvas/alignmentGuides";
import {
  allocateMultiPorts,
  computeCollisionFreeEdgeLabels,
  computeElkOrthogonalLayout,
} from "@/components/canvas/elkOrthogonalLayout";
import { useAnimatedNodes } from "@/components/canvas/useAnimatedNodes";
import {
  Button,
  IconButton,
  Input,
  SegmentedControl,
  Select,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  DomainGroupNode,
  type StudioDomainGroupNode,
} from "./DomainGroupNode";
import {
  StudioCanvasNodeCard,
  type StudioFlowNode,
} from "./NodeTypeCard";
import { PipelineViewCanvas } from "./PipelineViewCanvas";
import { computeSubordinateTierMap } from "./tierFolding";

export interface StudioEdgeData extends Record<string, unknown> {
  spec: EdgeTypeSpec;
  edgeIndex: number;
  selected: boolean;
  highlighted?: boolean;
  dimmed?: boolean;
  isDark: boolean;
  labelCenter?: { x: number; y: number };
  onSelectEdge: (index: number) => void;
  onHoverEdge?: (index: number | null) => void;
}

export type StudioFlowEdge = Edge<StudioEdgeData, "studioEdge">;
type AnyStudioRfNode = StudioFlowNode | StudioDomainGroupNode;

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
  const [edgePath, defaultLabelX, defaultLabelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    borderRadius: 8,
    offset: 28,
  });

  const spec = data?.spec;
  const selected = Boolean(data?.selected);
  const highlighted = Boolean(data?.highlighted);
  const isActive = selected || highlighted;
  const dimmed = Boolean(data?.dimmed) && !isActive;
  const isDark = Boolean(data?.isDark);
  const resolvedX = data?.labelCenter?.x ?? defaultLabelX;
  const resolvedY = data?.labelCenter?.y ?? defaultLabelY;
  const badgeText = spec
    ? CARDINALITY_SHORT[spec.cardinality] ?? "M:N"
    : "M:N";
  const strokeColor = isActive
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
          strokeWidth: isActive ? 2.3 : 1.5,
          opacity: dimmed ? 0.14 : 1,
          transition: "opacity 150ms ease, stroke-width 150ms ease, stroke 150ms ease",
        }}
      />
      {spec && (
        <EdgeLabelRenderer>
          <div
            style={{
              position: "absolute",
              transform: `translate(-50%, -50%) translate(${resolvedX}px,${resolvedY}px)`,
              pointerEvents: "all",
            }}
            className={cn("nodrag nopan transition-opacity duration-150", dimmed && "opacity-20")}
          >
            <button
              type="button"
              data-testid={`canvas-edge-${spec.name}`}
              onMouseEnter={() => data?.onHoverEdge?.(data.edgeIndex)}
              onMouseLeave={() => data?.onHoverEdge?.(null)}
              onClick={(e) => {
                e.stopPropagation();
                data?.onSelectEdge(data.edgeIndex);
              }}
              className={cn(
                "inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs backdrop-blur-xs transition-colors cursor-pointer whitespace-nowrap",
                selected
                  ? "border-xblue bg-xblue text-white"
                  : highlighted
                    ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                    : spec.maturity === "PROPOSED"
                      ? "border-dashed border-amber-500/70 bg-card/95 text-foreground hover:border-xblue"
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
  domainGroup: DomainGroupNode,
};

const EDGE_TYPES: EdgeTypes = {
  studioEdge: StudioRelationshipEdge,
};

export interface StudioCanvasProps {
  subgraphKey?: string;
  nodeTypes: NodeTypeSpec[];
  edgeTypes: EdgeTypeSpec[];
  workingDoc?: SchemaDoc;
  datasetNames?: readonly string[];
  issues: ValidationIssue[];
  consumptionByNodeType?: Record<string, NodeConsumptionCounts>;
  selectedNodeName: string | null;
  selectedEdgeIndex: number | null;
  selectedMappingIndex?: number | null;
  canUndo?: boolean;
  canRedo?: boolean;
  onUndo?: () => void;
  onRedo?: () => void;
  onSelectNode: (name: string | null) => void;
  onSelectEdge: (edgeIndex: number | null) => void;
  onSelectMapping?: (mappingIndex: number) => void;
  onCreateMapping?: (mapping: MappingSpec) => void;
  onUpdateNodePosition: (name: string, posX: number, posY: number) => void;
  onApplyLayoutPositions: (
    positions: Record<string, { x: number; y: number }>
  ) => void;
  onCreateNodeType: (name: string, displayName: string) => void;
  onCreateEdgeType: (edge: EdgeTypeSpec) => void;
  onConfirmNode?: (nodeName: string) => void;
  onOpenLineage?: (nodeName: string) => void;
  onOpenAppPackaging?: (nodeName: string) => void;
  isFullscreen?: boolean;
  onToggleFullscreen?: () => void;
}

const studioViewportCache = new Map<string, Viewport>();

function StudioCanvasInner({
  subgraphKey = "default",
  nodeTypes,
  edgeTypes,
  workingDoc,
  datasetNames = [],
  issues,
  consumptionByNodeType,
  selectedNodeName,
  selectedEdgeIndex,
  selectedMappingIndex = null,
  canUndo = false,
  canRedo = false,
  onUndo,
  onRedo,
  onSelectNode,
  onSelectEdge,
  onSelectMapping,
  onCreateMapping,
  onUpdateNodePosition,
  onApplyLayoutPositions,
  onCreateNodeType,
  onCreateEdgeType,
  onConfirmNode,
  onOpenLineage,
  onOpenAppPackaging,
  isFullscreen = false,
  onToggleFullscreen,
}: StudioCanvasProps) {
  const { tx, tEnum } = useI18n();
  const { isDark } = useTheme();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");
  const nodesInitialized = useNodesInitialized();
  const { fitView, setViewport } = useReactFlow();
  const canvasWrapperRef = useRef<HTMLDivElement | null>(null);
  const initializedSubgraphRef = useRef<string>("");
  const viewportReadyRef = useRef(false);
  const { animatePositions } = useAnimatedNodes(240);

  const nodeNamesKey = useMemo(
    () => nodeTypes.map((n) => n.name).sort().join(","),
    [nodeTypes]
  );

  useEffect(() => {
    if (initializedSubgraphRef.current !== subgraphKey) {
      viewportReadyRef.current = false;
    }
  }, [subgraphKey]);

  useEffect(() => {
    if (!nodesInitialized || !nodeNamesKey) return;
    if (initializedSubgraphRef.current === subgraphKey) return;
    initializedSubgraphRef.current = subgraphKey;
    const cached = studioViewportCache.get(subgraphKey);
    const t = setTimeout(() => {
      if (cached) {
        void setViewport(cached, { duration: 0 });
      } else {
        void fitView({ padding: 0.22, minZoom: 0.15, maxZoom: 0.95, duration: 0 });
      }
      viewportReadyRef.current = true;
    }, 40);
    return () => clearTimeout(t);
  }, [fitView, nodeNamesKey, nodesInitialized, setViewport, subgraphKey]);

  const [hoveredNodeId, setHoveredNodeId] = useState<string | null>(null);
  const [hoveredEdgeIdx, setHoveredEdgeIdx] = useState<number | null>(null);
  const [pinnedNodeId, setPinnedNodeId] = useState<string | null>(null);
  const [pinnedEdgeIdx, setPinnedEdgeIdx] = useState<number | null>(null);

  useEffect(() => {
    setHoveredNodeId(null);
    setHoveredEdgeIdx(null);
    setPinnedNodeId(null);
    setPinnedEdgeIdx(null);
  }, [nodeNamesKey]);

  const [viewMode, setViewMode] =
    useState<StudioCanvasViewMode>("ONTOLOGY_TOPOLOGY");
  const [groupByDomain, setGroupByDomain] = useState(false);
  const [collapsedDomains, setCollapsedDomains] = useState<Set<string>>(
    () => new Set()
  );
  const [foldedMasters, setFoldedMasters] = useState<Set<string>>(
    () => new Set()
  );
  const [marqueeMode, setMarqueeMode] = useState(false);

  const [quickAddNodeOpen, setQuickAddNodeOpen] = useState(false);
  const [newNodeName, setNewNodeName] = useState("");
  const [newNodeDisplay, setNewNodeDisplay] = useState("");
  const [quickAddEdgeOpen, setQuickAddEdgeOpen] = useState(false);
  const [newEdgeName, setNewEdgeName] = useState("SUPPLIES");
  const [newEdgeSrc, setNewEdgeSrc] = useState("");
  const [newEdgeDst, setNewEdgeDst] = useState("");
  const [layoutRunning, setLayoutRunning] = useState(false);

  const [helperGuides, setHelperGuides] = useState<{
    verticalLines: readonly VerticalGuideLine[];
    horizontalLines: readonly HorizontalGuideLine[];
  }>({ verticalLines: [], horizontalLines: [] });

  const draggingNodeIdRef = useRef<string | null>(null);
  const snappedPosRef = useRef<{ x: number; y: number } | null>(null);

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

  const { dependentsByMaster, masterByDependent } = useMemo(
    () => computeSubordinateTierMap(nodeTypes, edgeTypes),
    [nodeTypes, edgeTypes]
  );

  const hiddenByTierFold = useMemo(() => {
    const set = new Set<string>();
    for (const [depName, masterName] of Object.entries(masterByDependent)) {
      if (foldedMasters.has(masterName)) {
        set.add(depName);
      }
    }
    return set;
  }, [foldedMasters, masterByDependent]);

  const toggleDomainCollapse = useCallback((dg: string) => {
    setCollapsedDomains((prev) => {
      const next = new Set(prev);
      if (next.has(dg)) {
        next.delete(dg);
      } else {
        next.add(dg);
      }
      return next;
    });
  }, []);

  const toggleTierFold = useCallback((masterName: string) => {
    setFoldedMasters((prev) => {
      const next = new Set(prev);
      if (next.has(masterName)) {
        next.delete(masterName);
      } else {
        next.add(masterName);
      }
      return next;
    });
  }, []);

  const visibleNodeSpecs = useMemo(
    () => nodeTypes.filter((nt) => !hiddenByTierFold.has(nt.name)),
    [hiddenByTierFold, nodeTypes]
  );

  const visibleNodeNameSet = useMemo(
    () => new Set(visibleNodeSpecs.map((n) => n.name)),
    [visibleNodeSpecs]
  );

  const allNodeNameSet = useMemo(
    () => new Set(nodeTypes.map((n) => n.name)),
    [nodeTypes]
  );

  const activeEdgeInputs = useMemo(
    () =>
      edgeTypes
        .map((et, idx) => ({ et, idx }))
        .filter(
          ({ et }) =>
            visibleNodeNameSet.has(et.src_type_name) &&
            visibleNodeNameSet.has(et.dst_type_name)
        )
        .map(({ et, idx }) => ({
          id: `edge-${idx}-${et.src_type_name}-${et.name}-${et.dst_type_name}`,
          source: et.src_type_name,
          target: et.dst_type_name,
          et,
          idx,
        })),
    [edgeTypes, visibleNodeNameSet]
  );

  const { portsByNode, edgeBindings } = useMemo(
    () =>
      allocateMultiPorts(
        visibleNodeSpecs.map((n) => ({ id: n.name })),
        activeEdgeInputs.map((e) => ({
          id: e.id,
          source: e.source,
          target: e.target,
        }))
      ),
    [activeEdgeInputs, visibleNodeSpecs]
  );

  const focusHighlight = useMemo(() => {
    const nodes = new Set<string>();
    const edgeIndices = new Set<number>();
    const targetNode =
      hoveredNodeId ?? (hoveredEdgeIdx === null ? pinnedNodeId : null);
    const targetEdgeIdx =
      hoveredEdgeIdx ?? (hoveredNodeId === null ? pinnedEdgeIdx : null);

    if (targetNode) {
      nodes.add(targetNode);
      for (const { et, idx } of activeEdgeInputs) {
        if (et.src_type_name === targetNode || et.dst_type_name === targetNode) {
          nodes.add(et.src_type_name);
          nodes.add(et.dst_type_name);
          edgeIndices.add(idx);
        }
      }
      return { active: true, nodes, edgeIndices };
    }

    if (targetEdgeIdx !== null) {
      const hit = activeEdgeInputs.find((e) => e.idx === targetEdgeIdx);
      if (hit) {
        nodes.add(hit.et.src_type_name);
        nodes.add(hit.et.dst_type_name);
        edgeIndices.add(hit.idx);
        return { active: true, nodes, edgeIndices };
      }
    }

    return { active: false, nodes, edgeIndices };
  }, [
    activeEdgeInputs,
    hoveredEdgeIdx,
    hoveredNodeId,
    pinnedEdgeIdx,
    pinnedNodeId,
  ]);

  // Preserve saved finite node coordinates verbatim so dragging one node never pushes sibling nodes
  const computedRfNodes: AnyStudioRfNode[] = useMemo(() => {
    const CARD_MIN_DX = 384;
    const CARD_MIN_DY = 224;
    const placed: Array<{ x: number; y: number }> = [];
    const absPositions = new Map<string, { x: number; y: number }>();

    for (const nt of visibleNodeSpecs) {
      if (Number.isFinite(nt.pos_x) && Number.isFinite(nt.pos_y)) {
        const pos = { x: nt.pos_x, y: nt.pos_y };
        placed.push(pos);
        absPositions.set(nt.name, pos);
      }
    }

    visibleNodeSpecs.forEach((nt, idx) => {
      if (absPositions.has(nt.name)) return;
      let x = 96 + (idx % 3) * 420;
      let y = 96 + Math.floor(idx / 3) * 244;
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
      absPositions.set(nt.name, { x, y });
    });

    const domainGroups = new Map<string, NodeTypeSpec[]>();
    if (groupByDomain) {
      for (const nt of visibleNodeSpecs) {
        const dg = nt.domain_group?.trim();
        if (dg) {
          const list = domainGroups.get(dg) ?? [];
          list.push(nt);
          domainGroups.set(dg, list);
        }
      }
    }

    const outNodes: AnyStudioRfNode[] = [];
    const domainOrigins = new Map<string, { x: number; y: number }>();

    let domainIdx = 0;
    for (const [dg, members] of domainGroups.entries()) {
      const collapsed = collapsedDomains.has(dg);
      const memberCoords = members.map(
        (m) => absPositions.get(m.name) ?? { x: 96, y: 96 }
      );
      const minX = Math.min(...memberCoords.map((c) => c.x)) - 28;
      const minY = Math.min(...memberCoords.map((c) => c.y)) - 52;
      const maxX = Math.max(...memberCoords.map((c) => c.x + 240)) + 28;
      const maxY = Math.max(...memberCoords.map((c) => c.y + 196)) + 28;

      const gx = collapsed ? 96 + domainIdx * 320 : minX;
      const gy = collapsed ? 96 : minY;
      const gw = collapsed ? 264 : Math.max(304, maxX - minX);
      const gh = collapsed ? 92 : Math.max(248, maxY - minY);
      domainIdx += 1;

      domainOrigins.set(dg, { x: gx, y: gy });
      outNodes.push({
        id: `domain:${dg}`,
        type: "domainGroup",
        position: { x: gx, y: gy },
        style: { width: gw, height: gh },
        data: {
          domainGroup: dg,
          memberCount: members.length,
          collapsed,
          width: gw,
          height: gh,
          onToggleCollapse: toggleDomainCollapse,
        },
      });
    }

    for (const nt of visibleNodeSpecs) {
      const dg = groupByDomain ? nt.domain_group?.trim() : undefined;
      if (dg && collapsedDomains.has(dg)) {
        continue;
      }
      const abs = absPositions.get(nt.name) ?? { x: 96, y: 96 };
      const parentOrigin = dg ? domainOrigins.get(dg) : undefined;
      const pos = parentOrigin
        ? {
            x: Math.max(20, abs.x - parentOrigin.x),
            y: Math.max(48, abs.y - parentOrigin.y),
          }
        : abs;
      const st = issueStatsByNode[nt.name] ?? { count: 0, hasError: false };
      const alloc = portsByNode[nt.name] ?? {
        inPorts: ["in-0"],
        outPorts: ["out-0"],
      };
      const deps = dependentsByMaster[nt.name] ?? [];

      outNodes.push({
        id: nt.name,
        type: "studioNode",
        position: pos,
        ...(parentOrigin
          ? {
              parentId: `domain:${dg}`,
              extent: "parent" as const,
            }
          : {}),
        data: {
          spec: nt,
          selected: selectedNodeName === nt.name,
          highlighted: focusHighlight.active && focusHighlight.nodes.has(nt.name),
          dimmed: focusHighlight.active && !focusHighlight.nodes.has(nt.name),
          issueCount: st.count,
          hasError: st.hasError,
          inPorts: alloc.inPorts,
          outPorts: alloc.outPorts,
          dependentSpecs: deps,
          tierFolded: foldedMasters.has(nt.name),
          onToggleTierFold: toggleTierFold,
          onSelectFoldedSpec: (specName: string) => {
            onSelectEdge(null);
            onSelectNode(specName);
          },
          consumption: consumptionByNodeType?.[nt.name] ?? null,
          onConfirmNode,
          onDoubleClickNode: onOpenLineage,
          onOpenAppPackaging,
        },
      });
    }

    return outNodes;
  }, [
    collapsedDomains,
    consumptionByNodeType,
    dependentsByMaster,
    focusHighlight,
    foldedMasters,
    groupByDomain,
    issueStatsByNode,
    onConfirmNode,
    onOpenAppPackaging,
    onOpenLineage,
    onSelectEdge,
    onSelectNode,
    portsByNode,
    selectedNodeName,
    toggleDomainCollapse,
    toggleTierFold,
    visibleNodeSpecs,
  ]);

  const [liveNodes, setLiveNodes] = useState<AnyStudioRfNode[]>(computedRfNodes);
  const liveNodesRef = useRef<AnyStudioRfNode[]>(computedRfNodes);
  liveNodesRef.current = liveNodes;

  useEffect(() => {
    setLiveNodes((prev) => {
      const prevMap = new Map(prev.map((n) => [n.id, n]));
      return computedRfNodes.map((nextNode) => {
        const existing = prevMap.get(nextNode.id);
        if (
          existing &&
          (existing.dragging || draggingNodeIdRef.current === nextNode.id)
        ) {
          return {
            ...nextNode,
            position: existing.position,
            measured: existing.measured,
            selected: existing.selected,
            dragging: existing.dragging,
          } as AnyStudioRfNode;
        }
        if (existing) {
          return {
            ...nextNode,
            measured: existing.measured,
            selected: existing.selected,
          } as AnyStudioRfNode;
        }
        return nextNode;
      });
    });
  }, [computedRfNodes]);

  const handleNodesChange: OnNodesChange<AnyStudioRfNode> = useCallback(
    (changes) => {
      setLiveNodes((nds) => applyNodeChanges(changes, nds));
    },
    []
  );

  // Absolute bounding boxes of all rendered object cards for collision-free label routing
  const absoluteNodeBoxes = useMemo(() => {
    const parentPos = new Map<string, { x: number; y: number }>();
    for (const n of liveNodes) {
      if (n.type === "domainGroup") {
        parentPos.set(n.id, n.position);
      }
    }
    const map = new Map<string, MeasuredBox>();
    for (const n of liveNodes) {
      const p = n.parentId ? parentPos.get(n.parentId) : undefined;
      const absX = (p?.x ?? 0) + n.position.x;
      const absY = (p?.y ?? 0) + n.position.y;
      const defaultW = n.type === "domainGroup" ? 264 : 240;
      const defaultH = n.type === "domainGroup" ? 92 : 196;
      map.set(n.id, {
        id: n.id,
        x: absX,
        y: absY,
        width: n.measured?.width ?? defaultW,
        height: n.measured?.height ?? defaultH,
      });
    }
    return map;
  }, [liveNodes]);

  const flowEdges: StudioFlowEdge[] = useMemo(() => {
    const activeStroke = isDark ? "#3b82f6" : "#2563eb";
    const idleStroke = isDark ? "#71717a" : "#94a3b8";

    const resolveEndpointId = (typeName: string): string => {
      if (!groupByDomain) return typeName;
      const spec = nodeTypes.find((n) => n.name === typeName);
      const dg = spec?.domain_group?.trim();
      if (dg && collapsedDomains.has(dg)) {
        return `domain:${dg}`;
      }
      return typeName;
    };

    const resolvedEdges = activeEdgeInputs
      .map((entry) => {
        const sourceId = resolveEndpointId(entry.source);
        const targetId = resolveEndpointId(entry.target);
        return { ...entry, sourceId, targetId };
      })
      .filter((entry) => entry.sourceId !== entry.targetId);

    const cardObstacles: MeasuredBox[] = liveNodes
      .filter((n) => n.type === "studioNode")
      .map((n) => absoluteNodeBoxes.get(n.id))
      .filter((b): b is MeasuredBox => Boolean(b));

    const labelInputs = resolvedEdges.map((entry) => {
      const binding = edgeBindings[entry.id];
      const srcBox = absoluteNodeBoxes.get(entry.sourceId) ?? {
        id: entry.sourceId,
        x: 96,
        y: 96,
        width: 240,
        height: 180,
      };
      const dstBox = absoluteNodeBoxes.get(entry.targetId) ?? {
        id: entry.targetId,
        x: 480,
        y: 96,
        width: 240,
        height: 180,
      };
      const srcRatio = binding
        ? (binding.sourcePortIndex + 1) / (binding.sourcePortCount + 1)
        : 0.5;
      const dstRatio = binding
        ? (binding.targetPortIndex + 1) / (binding.targetPortCount + 1)
        : 0.5;
      return {
        id: entry.id,
        sourceX: srcBox.x + srcBox.width,
        sourceY: srcBox.y + srcBox.height * srcRatio,
        targetX: dstBox.x,
        targetY: dstBox.y + dstBox.height * dstRatio,
        pillWidth: Math.max(108, entry.et.name.length * 7.5 + 52),
        pillHeight: 26,
      };
    });

    const labelCenters = computeCollisionFreeEdgeLabels(
      labelInputs,
      cardObstacles,
      14
    );

    return resolvedEdges.map(({ id, et, idx, sourceId, targetId }) => {
      const binding = edgeBindings[id];
      const usePortHandles =
        sourceId === et.src_type_name && targetId === et.dst_type_name;
      const isSelectedEdge = selectedEdgeIndex === idx;
      const isHighlightedEdge =
        focusHighlight.active && focusHighlight.edgeIndices.has(idx);
      const isDimmedEdge =
        focusHighlight.active && !isHighlightedEdge && !isSelectedEdge;
      return {
        id,
        source: sourceId,
        target: targetId,
        ...(usePortHandles && binding
          ? {
              sourceHandle: binding.sourcePortId,
              targetHandle: binding.targetPortId,
            }
          : {}),
        type: "studioEdge",
        zIndex: isSelectedEdge || isHighlightedEdge ? 10 : 0,
        markerEnd: {
          type: MarkerType.ArrowClosed,
          color: isSelectedEdge || isHighlightedEdge ? activeStroke : idleStroke,
        },
        data: {
          spec: et,
          edgeIndex: idx,
          selected: isSelectedEdge,
          highlighted: isHighlightedEdge,
          dimmed: isDimmedEdge,
          isDark,
          labelCenter: labelCenters[id],
          onSelectEdge: (edgeIdx: number) => {
            onSelectNode(null);
            onSelectEdge(edgeIdx);
          },
          onHoverEdge: (edgeIdx: number | null) => {
            if (!draggingNodeIdRef.current) {
              setHoveredEdgeIdx(edgeIdx);
            }
          },
        },
      };
    });
  }, [
    absoluteNodeBoxes,
    activeEdgeInputs,
    collapsedDomains,
    edgeBindings,
    focusHighlight,
    groupByDomain,
    isDark,
    liveNodes,
    nodeTypes,
    onSelectEdge,
    onSelectNode,
    selectedEdgeIndex,
  ]);

  const selectedStudioNodes = useMemo(
    () =>
      liveNodes.filter(
        (n): n is StudioFlowNode => n.type === "studioNode" && Boolean(n.selected)
      ),
    [liveNodes]
  );

  const handleAlignSelection = useCallback(
    (action: AlignAction) => {
      if (!canEdit || selectedStudioNodes.length < 2) return;
      const boxes: MeasuredBox[] = selectedStudioNodes
        .map((n) => absoluteNodeBoxes.get(n.id))
        .filter((b): b is MeasuredBox => Boolean(b));
      if (boxes.length < 2) return;
      const nextMap = computeSelectionAlignment(boxes, action);
      onApplyLayoutPositions(nextMap);
    },
    [absoluteNodeBoxes, canEdit, onApplyLayoutPositions, selectedStudioNodes]
  );

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!canEdit || !connection.source || !connection.target) return;
      if (
        connection.source.startsWith("domain:") ||
        connection.target.startsWith("domain:")
      ) {
        return;
      }
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
    if (visibleNodeSpecs.length === 0 || layoutRunning) return;
    setLayoutRunning(true);
    try {
      const originPositions: Record<string, { x: number; y: number }> = {};
      for (const nt of visibleNodeSpecs) {
        const box = absoluteNodeBoxes.get(nt.name);
        originPositions[nt.name] = box
          ? { x: box.x, y: box.y }
          : { x: nt.pos_x, y: nt.pos_y };
      }

      const layoutInputs = visibleNodeSpecs.map((nt) => {
        const box = absoluteNodeBoxes.get(nt.name);
        return {
          id: nt.name,
          width: nodesInitialized && box ? box.width : 240,
          height: nodesInitialized && box ? box.height : 192,
          domainGroup: nt.domain_group ?? null,
        };
      });

      const res = await computeElkOrthogonalLayout(
        layoutInputs,
        activeEdgeInputs.map((e) => ({
          id: e.id,
          source: e.source,
          target: e.target,
          labelWidth: Math.max(108, e.et.name.length * 7.5 + 52),
          labelHeight: 26,
        })),
        { groupByDomain, collapsedDomains }
      );

      animatePositions(
        originPositions,
        res.nodePositions,
        (frame) => {
          setLiveNodes((prev) =>
            prev.map((n) => {
              if (n.type !== "studioNode" || n.parentId) return n;
              const f = frame[n.id];
              return f ? { ...n, position: { x: f.x, y: f.y } } : n;
            })
          );
        },
        () => {
          onApplyLayoutPositions({ ...res.nodePositions });
        }
      );
    } catch {
      const fallback: Record<string, { x: number; y: number }> = {};
      visibleNodeSpecs.forEach((nt, idx) => {
        fallback[nt.name] = {
          x: 96 + (idx % 3) * 440,
          y: 96 + Math.floor(idx / 3) * 240,
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

  const handleNodeDrag: OnNodeDrag<AnyStudioRfNode> = useCallback(
    (_event, draggedNode) => {
      if (draggedNode.type !== "studioNode") return;
      const curNodes = liveNodesRef.current;
      const parentOrigin = draggedNode.parentId
        ? curNodes.find((n) => n.id === draggedNode.parentId)?.position
        : undefined;
      const absX = (parentOrigin?.x ?? 0) + draggedNode.position.x;
      const absY = (parentOrigin?.y ?? 0) + draggedNode.position.y;
      const w = draggedNode.measured?.width ?? 240;
      const h = draggedNode.measured?.height ?? 192;

      const others: MeasuredBox[] = [];
      for (const n of curNodes) {
        if (n.type !== "studioNode" || n.id === draggedNode.id) continue;
        const pOrigin = n.parentId
          ? curNodes.find((p) => p.id === n.parentId)?.position
          : undefined;
        others.push({
          id: n.id,
          x: (pOrigin?.x ?? 0) + n.position.x,
          y: (pOrigin?.y ?? 0) + n.position.y,
          width: n.measured?.width ?? 240,
          height: n.measured?.height ?? 196,
        });
      }

      const snap = computeAlignmentGuides(
        { id: draggedNode.id, x: absX, y: absY, width: w, height: h },
        others,
        6
      );

      setHelperGuides({
        verticalLines: snap.verticalLines,
        horizontalLines: snap.horizontalLines,
      });

      const resolvedAbsX = snap.snappedX ?? absX;
      const resolvedAbsY = snap.snappedY ?? absY;
      snappedPosRef.current = { x: resolvedAbsX, y: resolvedAbsY };

      if (snap.snappedX !== null || snap.snappedY !== null) {
        const nextLocalX = resolvedAbsX - (parentOrigin?.x ?? 0);
        const nextLocalY = resolvedAbsY - (parentOrigin?.y ?? 0);
        setLiveNodes((prev) =>
          prev.map((n) =>
            n.id === draggedNode.id
              ? { ...n, position: { x: nextLocalX, y: nextLocalY } }
              : n
          )
        );
      }
    },
    []
  );

  const crossSubgraphEdges = useMemo(
    () =>
      edgeTypes
        .map((et, idx) => ({ et, idx }))
        .filter(
          ({ et }) =>
            !allNodeNameSet.has(et.src_type_name) ||
            !allNodeNameSet.has(et.dst_type_name)
        ),
    [allNodeNameSet, edgeTypes]
  );

  return (
    <div
      className="relative flex flex-col flex-1 min-h-[360px] w-full rounded-md border border-border bg-muted/15 overflow-hidden"
      data-testid="xbluestudio-canvas"
    >
      {/* Docked Canvas Header Toolbar (Zero Card Occlusion) */}
      <div className="z-10 flex items-center justify-between gap-2 px-2.5 py-1.5 border-b border-border bg-card/95 shrink-0">
        <div className="flex-1 min-w-0 flex flex-nowrap items-center gap-1.5 overflow-x-auto">
          <SegmentedControl<StudioCanvasViewMode>
            value={viewMode}
            onChange={setViewMode}
            items={[
              {
                key: "ONTOLOGY_TOPOLOGY",
                label: tEnum("ONTOLOGY_TOPOLOGY"),
                icon: <Blocks className="h-3.5 w-3.5" />,
                testId: "studio-view-ONTOLOGY_TOPOLOGY",
              },
              {
                key: "MAPPING_PIPELINE",
                label: tEnum("MAPPING_PIPELINE"),
                icon: <GitMerge className="h-3.5 w-3.5" />,
                testId: "studio-view-MAPPING_PIPELINE",
              },
            ]}
          />

          {canEdit && (onUndo || onRedo) && (
            <div className="inline-flex items-center gap-0.5 border-l border-border pl-1.5">
              <IconButton
                size="sm"
                data-testid="canvas-undo-btn"
                disabled={!canUndo}
                onClick={onUndo}
                title={tx("撤销", "Undo")}
                aria-label={tx("撤销", "Undo")}
              >
                <Undo2 className="h-3.5 w-3.5" />
              </IconButton>
              <IconButton
                size="sm"
                data-testid="canvas-redo-btn"
                disabled={!canRedo}
                onClick={onRedo}
                title={tx("重做", "Redo")}
                aria-label={tx("重做", "Redo")}
              >
                <Redo2 className="h-3.5 w-3.5" />
              </IconButton>
            </div>
          )}

          {viewMode === "ONTOLOGY_TOPOLOGY" && (
            <>
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
                      setNewEdgeDst(
                        nodeTypes[1]?.name ?? nodeTypes[0]?.name ?? ""
                      );
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
                disabled={layoutRunning || visibleNodeSpecs.length === 0}
              >
                <LayoutGrid className="h-3.5 w-3.5 text-xblue" />
                {layoutRunning
                  ? tx("排布中…", "Arranging…")
                  : tx("自动分层布局", "Auto Layout")}
              </Button>

              <Button
                variant={groupByDomain ? "primary" : "outline"}
                size="sm"
                data-testid="canvas-domain-group-toggle"
                onClick={() => setGroupByDomain((v) => !v)}
              >
                <Layers className="h-3.5 w-3.5" />
                {tx("按域分组", "Domain Lanes")}
              </Button>

              {canEdit && (
                <IconButton
                  size="sm"
                  active={marqueeMode}
                  data-testid="canvas-select-mode-btn"
                  onClick={() => setMarqueeMode((v) => !v)}
                  title={tx("框选对齐", "Marquee Select")}
                  aria-label={tx("框选对齐", "Marquee Select")}
                >
                  <MousePointerSquareDashed className="h-3.5 w-3.5" />
                </IconButton>
              )}

              {onOpenLineage && nodeTypes.length > 0 && (
                <Button
                  variant="outline"
                  size="sm"
                  data-testid="canvas-open-lineage-btn"
                  onClick={() => {
                    const target = selectedNodeName ?? nodeTypes[0]?.name;
                    if (target) {
                      onOpenLineage(target);
                    }
                  }}
                >
                  <GitBranch className="h-3.5 w-3.5 text-xblue" />
                  {tx("血缘视图", "Lineage")}
                </Button>
              )}
            </>
          )}
        </div>

        {viewMode === "ONTOLOGY_TOPOLOGY" && (
          <div className="flex items-center gap-1.5 shrink-0">
            {canEdit && (
              <SelectionAlignToolbar
                selectedCount={selectedStudioNodes.length}
                onAlign={handleAlignSelection}
              />
            )}
            {onToggleFullscreen && (
              <IconButton
                size="sm"
                active={isFullscreen}
                data-testid="canvas-fullscreen-btn"
                onClick={onToggleFullscreen}
                title={
                  isFullscreen
                    ? tx("退出沉浸全屏 (Esc)", "Exit Fullscreen (Esc)")
                    : tx("沉浸式全屏画布", "Fullscreen Canvas")
                }
                aria-label={
                  isFullscreen
                    ? tx("退出沉浸全屏", "Exit Fullscreen")
                    : tx("沉浸式全屏画布", "Fullscreen Canvas")
                }
              >
                {isFullscreen ? (
                  <Minimize2 className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Maximize2 className="h-3.5 w-3.5" />
                )}
              </IconButton>
            )}
          </div>
        )}
      </div>

      {viewMode === "MAPPING_PIPELINE" ? (
        <PipelineViewCanvas
          workingDoc={
            workingDoc ?? {
              node_types: nodeTypes,
              edge_types: edgeTypes,
              mappings: [],
            }
          }
          datasetNames={datasetNames}
          selectedNodeName={selectedNodeName}
          selectedMappingIndex={selectedMappingIndex}
          onSelectNode={(name) => onSelectNode(name)}
          onSelectMapping={(idx) => onSelectMapping?.(idx)}
          onCreateMapping={(m) => onCreateMapping?.(m)}
        />
      ) : (
        <>
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

          <div ref={canvasWrapperRef} className="relative flex-1 min-h-0 w-full">
            <ReactFlow
              nodes={liveNodes}
              edges={flowEdges}
              nodeTypes={NODE_TYPES}
              edgeTypes={EDGE_TYPES}
              onNodesChange={handleNodesChange}
              nodesDraggable={canEdit}
              nodesConnectable={canEdit}
              minZoom={0.12}
              maxZoom={2.5}
              zoomOnScroll
              zoomOnPinch
              zoomOnDoubleClick={false}
              panOnScroll={false}
              selectionOnDrag={marqueeMode}
              panOnDrag={marqueeMode ? [1, 2] : true}
              selectionMode={SelectionMode.Partial}
              multiSelectionKeyCode="Shift"
              onMoveEnd={(_event, viewport) => {
                if (viewportReadyRef.current) {
                  studioViewportCache.set(subgraphKey, viewport);
                }
              }}
              onNodeMouseEnter={(_event, node) => {
                if (node.type === "studioNode" && !draggingNodeIdRef.current) {
                  setHoveredNodeId(node.id);
                }
              }}
              onNodeMouseLeave={() => {
                setHoveredNodeId(null);
              }}
              onEdgeMouseEnter={(_event, edge) => {
                if (
                  typeof edge.data?.edgeIndex === "number" &&
                  !draggingNodeIdRef.current
                ) {
                  setHoveredEdgeIdx(edge.data.edgeIndex);
                }
              }}
              onEdgeMouseLeave={() => {
                setHoveredEdgeIdx(null);
              }}
              onNodeClick={(event, node) => {
                if (node.type !== "studioNode") return;
                if (event.shiftKey) return;
                setPinnedEdgeIdx(null);
                setPinnedNodeId((prev) => (prev === node.id ? null : node.id));
                onSelectEdge(null);
                onSelectNode(node.id);
              }}
              onNodeDoubleClick={(_event, node) => {
                if (node.type !== "studioNode") return;
                onSelectEdge(null);
                onSelectNode(node.id);
                onOpenLineage?.(node.id);
              }}
              onEdgeClick={(_event, edge) => {
                if (typeof edge.data?.edgeIndex === "number") {
                  const edgeIdx = edge.data.edgeIndex;
                  setPinnedNodeId(null);
                  setPinnedEdgeIdx((prev) => (prev === edgeIdx ? null : edgeIdx));
                  onSelectNode(null);
                  onSelectEdge(edgeIdx);
                }
              }}
              onPaneClick={() => {
                setHoveredNodeId(null);
                setHoveredEdgeIdx(null);
                setPinnedNodeId(null);
                setPinnedEdgeIdx(null);
                onSelectNode(null);
                onSelectEdge(null);
              }}
              onNodeDragStart={(_event, node) => {
                draggingNodeIdRef.current = node.id;
                snappedPosRef.current = null;
                setHoveredNodeId(null);
                setHoveredEdgeIdx(null);
              }}
              onNodeDrag={handleNodeDrag}
              onNodeDragStop={(_event, node) => {
                draggingNodeIdRef.current = null;
                setHelperGuides({ verticalLines: [], horizontalLines: [] });
                if (!canEdit || node.type !== "studioNode") return;
                const curNodes = liveNodesRef.current;
                const parentOrigin = node.parentId
                  ? curNodes.find((n) => n.id === node.parentId)?.position
                  : undefined;
                const finalX =
                  snappedPosRef.current?.x ??
                  (parentOrigin?.x ?? 0) + node.position.x;
                const finalY =
                  snappedPosRef.current?.y ??
                  (parentOrigin?.y ?? 0) + node.position.y;
                snappedPosRef.current = null;
                onUpdateNodePosition(
                  node.id,
                  Math.round(finalX),
                  Math.round(finalY)
                );
              }}
              onConnect={handleConnect}
              defaultViewport={studioViewportCache.get(subgraphKey)}
              proOptions={{ hideAttribution: true }}
            >
              <Background
                color={isDark ? "#27272a" : "#d4d4d8"}
                gap={16}
                size={1}
              />
              <HelperLinesOverlay
                verticalLines={helperGuides.verticalLines}
                horizontalLines={helperGuides.horizontalLines}
              />
              <Controls showInteractive={false} showFitView={false}>
                <ControlButton
                  onClick={() =>
                    void fitView({
                      padding: 0.22,
                      minZoom: 0.15,
                      maxZoom: 0.95,
                      duration: 200,
                    })
                  }
                  title={tx("适应画布居中", "Fit View")}
                  aria-label={tx("适应画布居中", "Fit View")}
                >
                  <LocateFixed className="h-3.5 w-3.5" />
                </ControlButton>
                {onToggleFullscreen && (
                  <ControlButton
                    onClick={onToggleFullscreen}
                    title={
                      isFullscreen
                        ? tx("退出全屏 (Esc)", "Exit Fullscreen (Esc)")
                        : tx("沉浸式全屏画布", "Fullscreen Canvas")
                    }
                    aria-label={
                      isFullscreen
                        ? tx("退出全屏", "Exit Fullscreen")
                        : tx("沉浸式全屏画布", "Fullscreen Canvas")
                    }
                  >
                    {isFullscreen ? (
                      <Minimize2 className="h-3.5 w-3.5" />
                    ) : (
                      <Maximize2 className="h-3.5 w-3.5" />
                    )}
                  </ControlButton>
                )}
              </Controls>
              {visibleNodeSpecs.length > 5 && (
                <MiniMap
                  pannable
                  zoomable
                  style={{
                    width: 132,
                    height: 84,
                    backgroundColor: isDark ? "#11141d" : "#ffffff",
                  }}
                  maskColor={
                    isDark
                      ? "rgba(9, 11, 16, 0.72)"
                      : "rgba(241, 245, 249, 0.72)"
                  }
                  nodeColor={(n) => {
                    const spec = (n.data as StudioFlowNode["data"] | undefined)
                      ?.spec;
                    return spec?.color || (isDark ? "#3b82f6" : "#2563eb");
                  }}
                  className="!bg-card !border !border-border !rounded-md"
                />
              )}
            </ReactFlow>
          </div>

          {/* Non-anchored cross-subgraph edges strip (outside ReactFlow viewport so 0 overlap) */}
          {crossSubgraphEdges.length > 0 && (
            <div className="shrink-0 border-t border-border bg-card/95 px-2.5 py-1 flex flex-wrap items-center gap-1 text-[11px]">
              <span className="text-muted-foreground">
                {tx("跨子图关系:", "Cross-subgraph:")}
              </span>
              {crossSubgraphEdges.map(({ et, idx }) => (
                <button
                  key={`${et.name}-${idx}`}
                  type="button"
                  data-testid={`canvas-cross-edge-${et.name}`}
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
        </>
      )}
    </div>
  );
}

export function StudioCanvas(props: StudioCanvasProps) {
  return (
    <ReactFlowProvider>
      <StudioCanvasInner {...props} />
    </ReactFlowProvider>
  );
}
