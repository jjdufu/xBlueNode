import { useMemo } from "react";
import {
  GridLayout,
  useContainerWidth,
  verticalCompactor,
  type Layout,
  type LayoutItem,
} from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import type {
  CanvasConfig,
  DrillPathItem,
  LayoutUpdateItem,
  WidgetDataResponse,
  WidgetRecord,
} from "@/lib/api/xblueview";
import type { SelectedNodeContext } from "./utils";
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
  fixedWidth?: number;
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  onLayoutChange?: (items: LayoutUpdateItem[]) => void;
  onRefreshWidget: (widgetId: string) => void;
  onEditWidget?: (widget: WidgetRecord) => void;
  onDeleteWidget?: (widgetId: string) => void;
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
  fixedWidth,
  selectedNodeId,
  filterValues,
  onLayoutChange,
  onRefreshWidget,
  onEditWidget,
  onDeleteWidget,
  onDrill,
  onPopDrill,
  onCrossFilter,
  onSelectNode,
  onActionSuccess,
}: DashboardGridProps) {
  const { width: measuredWidth, containerRef } = useContainerWidth({
    initialWidth: fixedWidth ?? 1200,
  });

  const activeWidth = fixedWidth ?? measuredWidth;
  const cols = canvas.cols > 0 ? canvas.cols : 24;
  const rowHeight = canvas.row_height > 0 ? canvas.row_height : 30;

  const layout = useMemo<Layout>(
    () =>
      widgets.map((w): LayoutItem => ({
        i: w.id,
        x: Math.max(0, Math.min(cols - 1, w.position?.x ?? 0)),
        y: Math.max(0, w.position?.y ?? 0),
        w: Math.max(2, Math.min(cols, w.position?.w ?? 6)),
        h: Math.max(2, w.position?.h ?? 4),
        minW: 2,
        minH: 2,
        static: !isEditing,
      })),
    [cols, isEditing, widgets]
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

  if (widgets.length === 0) {
    return (
      <div
        ref={containerRef}
        className="rounded-md border border-dashed border-border py-16 text-center text-xs text-muted-foreground"
        data-testid="dashboard-grid-empty"
      >
        当前驾驶舱暂无组件，请切换到「编辑布局」添加组件。
      </div>
    );
  }

  return (
    <div ref={containerRef} className="w-full" data-testid="dashboard-grid">
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
        {widgets.map((w) => (
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
              onRefresh={onRefreshWidget}
              onEdit={onEditWidget}
              onDelete={onDeleteWidget}
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
