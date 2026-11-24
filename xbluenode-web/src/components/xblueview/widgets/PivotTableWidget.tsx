import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { VTablePivotCanvas } from "@/components/analytics/VTablePivotCanvas";

export interface PivotTableWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onCellClick?: (
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
}

export function PivotTableWidget({
  widget,
  data,
  onCellClick,
}: PivotTableWidgetProps) {
  const { tx } = useI18n();
  if (!data || data.columns.length === 0) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        {tx("暂无透视表数据", "No pivot data")}
      </div>
    );
  }

  const initialDataBars = widget.options?.show_data_bars !== false;
  const initialHeatmap = Boolean(widget.options?.show_heatmap);
  const initialFreeze = widget.options?.freeze_headers !== false;

  return (
    <div
      className="h-full overflow-auto border border-border rounded"
      data-testid={`widget-pivot-${widget.id}`}
    >
      <VTablePivotCanvas
        widgetData={data}
        initialDataBars={initialDataBars}
        initialHeatmap={initialHeatmap}
        initialFreeze={initialFreeze}
        canvasHeight={160}
        maxTableHeightClass="max-h-56"
        onCellClick={onCellClick}
      />
    </div>
  );
}
