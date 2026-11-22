import {
  Download,
  GripVertical,
  Pencil,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  DrillPathItem,
  WidgetDataResponse,
  WidgetRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { Badge } from "@/components/ui/primitives";
import { DrillBreadcrumb } from "./DrillBreadcrumb";
import {
  getWidgetKindLabel,
  type KnownNodeSummary,
  type SelectedNodeContext,
} from "./utils";
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
  knownNodes?: Readonly<Record<string, KnownNodeSummary>>;
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
  knownNodes,
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
  const { isZh, tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canExport = can("EXPORT_DATA");
  const canEdit = can("VIEW_EDIT");

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
      {/* Header (entire bar acts as drag handle in edit mode) */}
      <div
        className={`px-3 py-1.5 border-b flex items-center justify-between gap-2 shrink-0 ${
          isScreen
            ? "border-zinc-800 bg-zinc-900/60"
            : "border-border bg-muted/20"
        } ${
          isEditing
            ? "widget-drag-handle cursor-grab active:cursor-grabbing select-none"
            : ""
        }`}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          {isEditing && (
            <span
              className="text-muted-foreground shrink-0"
              title={tx("拖拽移动", "Drag to move")}
            >
              <GripVertical className="h-3.5 w-3.5" />
            </span>
          )}
          <h4 className="text-xs font-semibold truncate">
            {widget.title || getWidgetKindLabel(widget.widget_kind, isZh)}
          </h4>
          {isEditing && (
            <Badge variant="outline" className="text-[10px]">
              {widget.widget_kind}
            </Badge>
          )}
        </div>

        <div
          className="flex items-center gap-0.5 shrink-0"
          onMouseDown={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
        >
          {!isScreen && (
            <span className="text-[10px] font-mono-tech text-muted-foreground hidden sm:inline mr-1">
              {tEnum(widget.refresh_mode)}
            </span>
          )}
          {!isEditing &&
            !readOnly &&
            !isScreen &&
            !isActionWidget &&
            canExport &&
            onExportWidget && (
              <button
                type="button"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => onExportWidget(widget)}
                className="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:text-xblue hover:bg-muted cursor-pointer"
                title={tx("导出", "Export")}
                aria-label={tx("导出", "Export")}
                data-testid={`export-widget-${widget.id}`}
              >
                <Download className="h-3.5 w-3.5" />
              </button>
            )}
          {!readOnly && (
            <button
              type="button"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => onRefresh(widget.id)}
              className="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
              title={tx("刷新", "Refresh")}
              aria-label={tx("刷新", "Refresh")}
              data-testid={`refresh-widget-${widget.id}`}
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${
                  loading ? "animate-spin text-xblue" : ""
                }`}
              />
            </button>
          )}
          {isEditing && canEdit && onEdit && (
            <button
              type="button"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => onEdit(widget)}
              className="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:text-xblue hover:bg-muted cursor-pointer"
              title={tx("编辑", "Edit")}
              aria-label={tx("编辑", "Edit")}
              data-testid={`edit-widget-${widget.id}`}
            >
              <Pencil className="h-3.5 w-3.5" />
            </button>
          )}
          {isEditing && canEdit && onDelete && (
            <button
              type="button"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => onDelete(widget.id)}
              className="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:text-red-600 dark:hover:text-red-400 hover:bg-muted cursor-pointer"
              title={tx("删除", "Delete")}
              aria-label={tx("删除", "Delete")}
              data-testid={`delete-widget-${widget.id}`}
            >
              <Trash2 className="h-3.5 w-3.5" />
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
      <div className="flex-1 min-h-0 p-2.5 overflow-y-auto relative">
        {error ? (
          <div className="h-full flex items-center justify-center text-xs text-muted-foreground text-center px-2">
            {tx(`加载失败: ${error}`, `Load error: ${error}`)}
          </div>
        ) : loading && !data ? (
          <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
            {tx("加载中…", "Loading…")}
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
            knownNodes={knownNodes}
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
