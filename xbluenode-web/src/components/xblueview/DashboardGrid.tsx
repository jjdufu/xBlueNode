import { useCallback, useMemo, useRef, useState } from "react";
import {
  GridLayout,
  useContainerWidth,
  verticalCompactor,
  type Compactor,
  type EventCallback,
  type Layout,
  type LayoutItem,
} from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import { useI18n } from "@/i18n";
import type {
  CanvasConfig,
  DrillPathItem,
  GridCompactMode,
  LayoutUpdateItem,
  SemanticZoneKind,
  WidgetDataResponse,
  WidgetRecord,
} from "@/lib/api/xblueview";
import {
  computeMagneticSnapGuides,
  resolveFreeFloatLayout,
  type GridLayoutBox,
  type MagneticSnapGuides,
} from "./logic";
import { formatSemanticZoneLabel } from "./PublishPreflightModal";
import {
  extractKnownNodesFromWidgetDataMap,
  type SelectedNodeContext,
} from "./utils";
import { WidgetFrame } from "./WidgetFrame";

export interface DashboardGridProps {
  widgets: WidgetRecord[];
  canvas: CanvasConfig;
  compactMode?: GridCompactMode;
  widgetDataMap: Record<string, WidgetDataResponse | null>;
  loadingMap: Record<string, boolean>;
  errorMap: Record<string, string | null>;
  drillMap: Record<string, DrillPathItem[]>;
  isEditing?: boolean;
  isDark?: boolean;
  isScreen?: boolean;
  readOnly?: boolean;
  hideActionWidgets?: boolean;
  fixedWidth?: number;
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  onLayoutChange?: (items: LayoutUpdateItem[]) => void;
  onRefreshWidget: (widgetId: string) => void;
  onEditWidget?: (widget: WidgetRecord) => void;
  onDeleteWidget?: (widgetId: string) => void;
  onExportWidget?: (widget: WidgetRecord) => void;
  onDrill?: (
    widgetId: string,
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
  onPopDrill?: (widgetId: string, level: number) => void;
  onCrossFilter?: (filterKey: string, value: unknown) => void;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onActionSuccess?: () => void;
}

const GRID_MARGIN_X = 12;
const GRID_MARGIN_Y = 12;

export function DashboardGrid({
  widgets,
  canvas,
  compactMode,
  widgetDataMap,
  loadingMap,
  errorMap,
  drillMap,
  isEditing = false,
  isDark = false,
  isScreen = false,
  readOnly = false,
  hideActionWidgets = false,
  fixedWidth,
  selectedNodeId,
  filterValues,
  onLayoutChange,
  onRefreshWidget,
  onEditWidget,
  onDeleteWidget,
  onExportWidget,
  onDrill,
  onPopDrill,
  onCrossFilter,
  onSelectNode,
  onActionSuccess,
}: DashboardGridProps) {
  const { isZh, tx } = useI18n();
  const { width: measuredWidth, containerRef } = useContainerWidth({
    initialWidth: fixedWidth ?? 1200,
  });

  const activeItemIdRef = useRef<string | null>(null);
  const dragBaselineRef = useRef<Map<string, GridLayoutBox>>(new Map());
  const [snapGuides, setSnapGuides] = useState<MagneticSnapGuides>({
    verticalCols: [],
    horizontalRows: [],
  });

  const knownNodes = useMemo(
    () => extractKnownNodesFromWidgetDataMap(widgetDataMap),
    [widgetDataMap]
  );

  const visibleWidgets = useMemo(
    () =>
      hideActionWidgets
        ? widgets.filter(
            (w) =>
              w.widget_kind !== "ACTION_BUTTON" &&
              w.widget_kind !== "ACTION_FORM"
          )
        : widgets,
    [hideActionWidgets, widgets]
  );

  const activeWidth = Math.max(320, fixedWidth ?? measuredWidth);
  const isFiveZone = canvas.zone_layout === "STANDARD_FIVE_ZONE";
  const cols = isFiveZone
    ? canvas.cols > 0 && canvas.cols <= 12
      ? canvas.cols
      : 12
    : canvas.cols > 0
      ? canvas.cols
      : 24;
  const rowHeight = canvas.row_height > 0 ? canvas.row_height : 30;
  const effectiveCompactMode: GridCompactMode =
    compactMode ?? canvas.compact_mode ?? "VERTICAL_COMPACT";

  const colUnitWidth = Math.max(
    8,
    (activeWidth - (cols - 1) * GRID_MARGIN_X) / cols
  );
  const rowPitchPx = rowHeight + GRID_MARGIN_Y;

  const fiveZoneRegions: ReadonlyArray<{
    zone: SemanticZoneKind;
    colSpanClass: string;
    coords: string;
  }> = [
    { zone: "CONTROL", colSpanClass: "col-span-12", coords: "12×2" },
    { zone: "KPI", colSpanClass: "col-span-12", coords: "12×3" },
    { zone: "SCENE", colSpanClass: "col-span-7", coords: "7×9" },
    { zone: "INSIGHT", colSpanClass: "col-span-5", coords: "5×9" },
    { zone: "DECISION", colSpanClass: "col-span-12", coords: "12×5" },
  ];

  const renderFiveZoneHeaderStrip = () => {
    if (!isEditing || !isFiveZone) return null;
    return (
      <div
        className="grid grid-cols-12 gap-1.5 mb-2.5"
        data-testid="five-zone-blueprint-strip"
      >
        {fiveZoneRegions.map(({ zone, colSpanClass, coords }) => (
          <div
            key={zone}
            data-testid={`semantic-zone-header-${zone}`}
            className={`${colSpanClass} flex items-center justify-between rounded border border-dashed border-xblue/35 bg-xblue-soft/30 px-2.5 py-1 text-[11px]`}
          >
            <span className="font-medium text-xblue">
              {formatSemanticZoneLabel(zone, isZh)}
            </span>
            <span className="font-mono-tech text-[10px] text-muted-foreground tabular-nums">
              {coords}
            </span>
          </div>
        ))}
      </div>
    );
  };

  const layout = useMemo<Layout>(
    () =>
      visibleWidgets.map(
        (w): LayoutItem => ({
          i: w.id,
          x: Math.max(0, Math.min(cols - 1, w.position?.x ?? 0)),
          y: Math.max(0, w.position?.y ?? 0),
          w: Math.max(2, Math.min(cols, w.position?.w ?? 6)),
          h: Math.max(2, w.position?.h ?? 4),
          minW: 2,
          minH: 2,
          static: !isEditing,
        })
      ),
    [cols, isEditing, visibleWidgets]
  );

  const freeFloatCompactor = useMemo<Compactor>(
    () => ({
      type: null,
      allowOverlap: false,
      preventCollision: true,
      compact(nextLayout: Layout, gridCols: number): Layout {
        return resolveFreeFloatLayout(
          nextLayout,
          gridCols,
          activeItemIdRef.current,
          dragBaselineRef.current
        );
      },
    }),
    []
  );

  const activeCompactor =
    effectiveCompactMode === "FREE_FLOAT"
      ? freeFloatCompactor
      : verticalCompactor;

  const captureBaseline = useCallback(
    (curLayout: Layout, activeItem: LayoutItem | null) => {
      activeItemIdRef.current = activeItem?.i ?? null;
      const snap = new Map<string, GridLayoutBox>();
      for (const item of curLayout) {
        snap.set(item.i, {
          i: item.i,
          x: item.x,
          y: item.y,
          w: item.w,
          h: item.h,
        });
      }
      dragBaselineRef.current = snap;
    },
    []
  );

  const handleInteractionStart: EventCallback = useCallback(
    (curLayout, oldItem, newItem) => {
      captureBaseline(curLayout, oldItem ?? newItem);
      const target = newItem ?? oldItem;
      setSnapGuides(computeMagneticSnapGuides(target, curLayout));
    },
    [captureBaseline]
  );

  const handleInteractionMove: EventCallback = useCallback(
    (curLayout, _oldItem, newItem, placeholder) => {
      const target = placeholder ?? newItem;
      if (target) {
        activeItemIdRef.current = target.i;
      }
      setSnapGuides(computeMagneticSnapGuides(target, curLayout));
    },
    []
  );

  const clearInteraction = useCallback(() => {
    activeItemIdRef.current = null;
    dragBaselineRef.current = new Map();
    setSnapGuides({ verticalCols: [], horizontalRows: [] });
  }, []);

  const handleInteractionStop: EventCallback = useCallback(
    (curLayout) => {
      const normalized =
        effectiveCompactMode === "FREE_FLOAT"
          ? resolveFreeFloatLayout(
              curLayout,
              cols,
              activeItemIdRef.current,
              dragBaselineRef.current
            )
          : curLayout;
      clearInteraction();
      if (isEditing && onLayoutChange) {
        onLayoutChange(
          normalized.map((item) => ({
            id: item.i,
            position: {
              x: item.x,
              y: item.y,
              w: item.w,
              h: item.h,
            },
          }))
        );
      }
    },
    [clearInteraction, cols, effectiveCompactMode, isEditing, onLayoutChange]
  );

  const handleRglLayoutChange = (nextLayout: Layout) => {
    if (!isEditing || !onLayoutChange) return;
    const resolved =
      effectiveCompactMode === "FREE_FLOAT"
        ? resolveFreeFloatLayout(
            nextLayout,
            cols,
            activeItemIdRef.current,
            dragBaselineRef.current
          )
        : nextLayout;
    const updates: LayoutUpdateItem[] = resolved.map((item) => ({
      id: item.i,
      position: {
        x: item.x,
        y: item.y,
        w: item.w,
        h: item.h,
      },
    }));
    onLayoutChange(updates);
  };

  if (visibleWidgets.length === 0) {
    return (
      <div ref={containerRef} className="w-full space-y-2">
        {renderFiveZoneHeaderStrip()}
        <div
          className="rounded-md border border-dashed border-border py-16 text-center text-xs text-muted-foreground"
          data-testid="dashboard-grid-empty"
        >
          {readOnly
            ? tx("当前看板暂无可展示组件", "No widgets available")
            : tx("当前驾驶舱暂无组件", "No widgets configured")}
        </div>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className="w-full"
      data-testid="dashboard-grid"
      data-compact-mode={effectiveCompactMode}
      data-cols={cols}
    >
      {renderFiveZoneHeaderStrip()}

      <div className="relative w-full min-h-[240px]">
        {/* Visible 12/24-Column CSS Dashed Cell Tracks in Edit Mode */}
        {isEditing && (
          <div
            aria-hidden="true"
            data-testid="dashboard-grid-tracks"
            data-track-cols={cols}
            style={{
              gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
              columnGap: `${GRID_MARGIN_X}px`,
              backgroundImage: `linear-gradient(to bottom, transparent ${rowHeight}px, color-mix(in srgb, var(--color-border) 55%, transparent) ${rowHeight}px, color-mix(in srgb, var(--color-border) 55%, transparent) ${rowHeight + 1}px, transparent ${rowHeight + 1}px)`,
              backgroundSize: `100% ${rowPitchPx}px`,
            }}
            className="pointer-events-none absolute inset-0 z-0 grid rounded-md"
          >
            {Array.from({ length: cols }, (_, colIdx) => (
              <div
                key={colIdx}
                data-testid={`dashboard-grid-col-track-${colIdx}`}
                className="h-full rounded-xs border border-dashed border-border/55 bg-muted/10"
              />
            ))}
          </div>
        )}

        {/* Magnetic Alignment Snap Lines (1.5px #2563eb) */}
        {isEditing &&
          snapGuides.verticalCols.map((colEdge) => {
            const leftPx =
              colEdge <= 0
                ? 0
                : colEdge >= cols
                  ? Math.max(0, activeWidth - 1.5)
                  : colEdge * (colUnitWidth + GRID_MARGIN_X) -
                    GRID_MARGIN_X / 2;
            return (
              <div
                key={`vsnap-${colEdge}`}
                data-testid="grid-snap-line-vertical"
                data-snap-col={colEdge}
                style={{ left: `${leftPx}px`, width: "1.5px" }}
                className="pointer-events-none absolute inset-y-0 z-30 bg-[#2563eb]"
              />
            );
          })}

        {isEditing &&
          snapGuides.horizontalRows.map((rowEdge) => {
            const topPx =
              rowEdge <= 0
                ? 0
                : rowEdge * rowPitchPx - GRID_MARGIN_Y / 2;
            return (
              <div
                key={`hsnap-${rowEdge}`}
                data-testid="grid-snap-line-horizontal"
                data-snap-row={rowEdge}
                style={{ top: `${topPx}px`, height: "1.5px" }}
                className="pointer-events-none absolute inset-x-0 z-30 bg-[#2563eb]"
              />
            );
          })}

        <GridLayout
          className="relative z-10"
          width={activeWidth}
          layout={layout}
          gridConfig={{
            cols,
            rowHeight,
            margin: [GRID_MARGIN_X, GRID_MARGIN_Y],
            containerPadding: [0, 0],
          }}
          dragConfig={{
            enabled: isEditing,
            handle: ".widget-drag-handle",
          }}
          resizeConfig={{
            enabled: isEditing,
            handles: ["n", "s", "e", "w", "ne", "nw", "se", "sw"],
          }}
          compactor={activeCompactor}
          onDragStart={handleInteractionStart}
          onDrag={handleInteractionMove}
          onDragStop={handleInteractionStop}
          onResizeStart={handleInteractionStart}
          onResize={handleInteractionMove}
          onResizeStop={handleInteractionStop}
          onLayoutChange={handleRglLayoutChange}
        >
          {visibleWidgets.map((w) => (
            <div key={w.id} className="h-full w-full">
              <WidgetFrame
                widget={w}
                data={widgetDataMap[w.id] ?? null}
                loading={loadingMap[w.id] ?? false}
                error={errorMap[w.id] ?? null}
                isEditing={isEditing}
                isDark={isDark}
                isScreen={isScreen}
                readOnly={readOnly}
                drillPath={drillMap[w.id] ?? []}
                selectedNodeId={selectedNodeId}
                filterValues={filterValues}
                knownNodes={knownNodes}
                onRefresh={onRefreshWidget}
                onEdit={onEditWidget}
                onDelete={onDeleteWidget}
                onExportWidget={onExportWidget}
                onDrill={onDrill}
                onPopDrill={onPopDrill}
                onCrossFilter={onCrossFilter}
                onSelectNode={onSelectNode}
                onActionSuccess={onActionSuccess}
              />
            </div>
          ))}
        </GridLayout>
      </div>
    </div>
  );
}
