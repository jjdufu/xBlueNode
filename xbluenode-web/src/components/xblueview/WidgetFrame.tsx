import { Download, GripVertical, Pencil, RefreshCw, Trash2 } from "lucide-react";
import type {
  DrillPathItem,
  WidgetDataResponse,
  WidgetRecord,
} from "@/lib/api/xblueview";
import { Badge } from "@/components/ui/primitives";
import { DrillBreadcrumb } from "./DrillBreadcrumb";
import { WIDGET_KIND_LABELS, type SelectedNodeContext } from "./utils";
import { WidgetRenderer } from "./widgets/WidgetRenderer";

export interface WidgetFrameProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  loading?: boolean;
  error?: string | null;
  isEditing?: boolean;
  isDark?: boolean;
  isScreen?: boolean;
  readOnly?: boolean;
  drillPath: DrillPathItem[];
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  onRefresh: (widgetId: string) => void;
  onEdit?: (widget: WidgetRecord) => void;
  onDelete?: (widgetId: string) => void;
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

export function WidgetFrame({
  widget,
  data,
  loading = false,
  error = null,
  isEditing = false,
  isDark = false,
  isScreen = false,
  readOnly = false,
  drillPath,
  selectedNodeId,
  filterValues,
  onRefresh,
  onEdit,
  onDelete,
  onExportWidget,
  onDrill,
  onPopDrill,
  onCrossFilter,
  onSelectNode,
  onActionSuccess,
}: WidgetFrameProps) {
  const isActionWidget =
    widget.widget_kind === "ACTION_BUTTON" ||
    widget.widget_kind === "ACTION_FORM";

  return (
    <div
      className={`h-full w-full flex flex-col rounded-md border overflow-hidden ${
        isScreen
          ? "border-zinc-800 bg-zinc-950/90 text-zinc-100"
          : "border-border bg-card text-card-foreground"
      }`}
      data-testid={`widget-frame-${widget.id}`}
      data-widget-kind={widget.widget_kind}
    >
      {/* Header */}
      <div
        className={`px-3 py-2 border-b flex items-center justify-between gap-2 shrink-0 ${
          isScreen ? "border-zinc-800 bg-zinc-900/60" : "border-border bg-muted/20"
        }`}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          {isEditing && (
            <span
              className="widget-drag-handle cursor-grab active:cursor-grabbing text-muted-foreground hover:text-foreground shrink-0"
              title="拖拽调整位置"
            >
              <GripVertical className="h-3.5 w-3.5" />
            </span>
          )}
          <h4 className="text-xs font-semibold truncate">
            {widget.title || WIDGET_KIND_LABELS[widget.widget_kind]}
          </h4>
          {isEditing && (
            <Badge variant="outline" className="text-[10px]">
              {widget.widget_kind}
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-1 shrink-0">
          {!isScreen && (
            <span className="text-[10px] font-mono-tech text-muted-foreground hidden sm:inline">
              {widget.refresh_mode}
            </span>
          )}
          {!isEditing &&
            !readOnly &&
            !isScreen &&
            !isActionWidget &&
            onExportWidget && (
              <button
                type="button"
                onClick={() => onExportWidget(widget)}
                className="rounded p-1 text-muted-foreground hover:text-xblue hover:bg-muted cursor-pointer"
                title="导出组件"
                data-testid={`export-widget-${widget.id}`}
              >
                <Download className="h-3 w-3" />
              </button>
            )}
          {!readOnly && (
            <button
              type="button"
              onClick={() => onRefresh(widget.id)}
              className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
              title="刷新组件"
              data-testid={`refresh-widget-${widget.id}`}
            >
              <RefreshCw
                className={`h-3 w-3 ${loading ? "animate-spin text-xblue" : ""}`}
              />
            </button>
          )}
          {isEditing && onEdit && (
            <button
              type="button"
              onClick={() => onEdit(widget)}
              className="rounded p-1 text-muted-foreground hover:text-xblue hover:bg-muted cursor-pointer"
              title="编辑组件"
              data-testid={`edit-widget-${widget.id}`}
            >
              <Pencil className="h-3 w-3" />
            </button>
          )}
          {isEditing && onDelete && (
            <button
              type="button"
              onClick={() => onDelete(widget.id)}
              className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
              title="删除组件"
              data-testid={`delete-widget-${widget.id}`}
            >
              <Trash2 className="h-3 w-3" />
            </button>
          )}
        </div>
      </div>

      {/* Drill Breadcrumb */}
      {drillPath.length > 0 && onPopDrill && (
        <div className="px-3 py-1.5 border-b border-border/60 bg-muted/10 shrink-0">
          <DrillBreadcrumb
            drillPath={drillPath}
            onPopToLevel={(lvl) => onPopDrill(widget.id, lvl)}
          />
        </div>
      )}

      {/* Body */}
      <div className="flex-1 min-h-0 p-2.5 overflow-hidden relative">
        {error ? (
          <div className="h-full flex items-center justify-center text-xs text-muted-foreground text-center px-2">
            加载失败: {error}
          </div>
        ) : loading && !data ? (
          <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
            正在加载数据...
          </div>
        ) : (
          <WidgetRenderer
            widget={widget}
            data={data}
            isDark={isDark || isScreen}
            isEditing={isEditing}
            readOnly={readOnly}
            selectedNodeId={selectedNodeId}
            filterValues={filterValues}
            onDrill={onDrill}
            onCrossFilter={onCrossFilter}
            onSelectNode={onSelectNode}
            onActionSuccess={onActionSuccess}
          />
        )}
      </div>
    </div>
  );
}
