import { useMemo } from "react";
import {
  GridLayout,
  useContainerWidth,
  verticalCompactor,
  type Layout,
  type LayoutItem,
} from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import { useI18n } from "@/i18n";
import type {
  CanvasConfig,
  DrillPathItem,
  LayoutUpdateItem,
  SemanticZoneKind,
  WidgetDataResponse,
  WidgetRecord,
} from "@/lib/api/xblueview";
import { formatSemanticZoneLabel } from "./PublishPreflightModal";
import {
  extractKnownNodesFromWidgetDataMap,
  type SelectedNodeContext,
} from "./utils";
import { WidgetFrame } from "./WidgetFrame";

export interface DashboardGridProps {
  widgets: WidgetRecord[];
  canvas: CanvasConfig;
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

export function DashboardGrid({
  widgets,
  canvas,
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
  const { isZh } = useI18n();
  const { tx } = useI18n();
  const { width: measuredWidth, containerRef } = useContainerWidth({
    initialWidth: fixedWidth ?? 1200,
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

  const activeWidth = fixedWidth ?? measuredWidth;
  const isFiveZone = canvas.zone_layout === "STANDARD_FIVE_ZONE";
  const cols = isFiveZone
    ? canvas.cols > 0 && canvas.cols <= 12
      ? canvas.cols
      : 12
    : canvas.cols > 0
      ? canvas.cols
      : 24;
  const rowHeight = canvas.row_height > 0 ? canvas.row_height : 30;

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

  const handleRglLayoutChange = (nextLayout: Layout) => {
    if (!isEditing || !onLayoutChange) return;
    const updates: LayoutUpdateItem[] = nextLayout.map((item) => ({
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
    <div ref={containerRef} className="w-full" data-testid="dashboard-grid">
      {renderFiveZoneHeaderStrip()}
      <GridLayout
        width={activeWidth}
        layout={layout}
        gridConfig={{
          cols,
          rowHeight,
          margin: [12, 12],
          containerPadding: [0, 0],
        }}
        dragConfig={{
          enabled: isEditing,
          handle: ".widget-drag-handle",
        }}
        resizeConfig={{
          enabled: isEditing,
          handles: ["se", "e", "s"],
        }}
        compactor={verticalCompactor}
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
  );
}
