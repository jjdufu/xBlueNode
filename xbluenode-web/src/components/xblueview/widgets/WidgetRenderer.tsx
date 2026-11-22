import { useMemo } from "react";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { ActionWidget } from "../ActionWidget";
import { FloorPlanWidget } from "../FloorPlanWidget";
import { MapWidget } from "../MapWidget";
import { Scene3dWidget } from "../Scene3dWidget";
import {
  parseCrossFilter,
  type KnownNodeSummary,
  type SelectedNodeContext,
} from "../utils";
import { AlertListWidget } from "./AlertListWidget";
import { EChartsWidget } from "./EChartsWidget";
import { KpiWidget } from "./KpiWidget";
import { MarkdownWidget } from "./MarkdownWidget";
import { PivotTableWidget } from "./PivotTableWidget";
import { SimCompareWidget } from "./SimCompareWidget";
import { TableWidget } from "./TableWidget";

export interface WidgetRendererProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  isEditing?: boolean;
  readOnly?: boolean;
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  knownNodes?: Readonly<Record<string, KnownNodeSummary>>;
  onDrill?: (
    widgetId: string,
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
  onCrossFilter?: (filterKey: string, value: unknown) => void;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onActionSuccess?: () => void;
}

export function WidgetRenderer({
  widget,
  data,
  isDark = false,
  isEditing = false,
  readOnly = false,
  selectedNodeId,
  filterValues,
  knownNodes,
  onDrill,
  onCrossFilter,
  onSelectNode,
  onActionSuccess,
}: WidgetRendererProps) {
  const crossFilter = parseCrossFilter(widget.options);

  const nodeLabelMap = useMemo(() => {
    if (!knownNodes) return undefined;
    const out: Record<string, string> = {};
    for (const [id, info] of Object.entries(knownNodes)) {
      out[id] =
        info.nodeKey && info.nodeKey !== info.title
          ? `${info.title} (${info.nodeKey})`
          : info.title;
    }
    return out;
  }, [knownNodes]);

  const handleItemClick = (
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => {
    if (crossFilter && onCrossFilter) {
      onCrossFilter(crossFilter.target_filter_key, value);
    }
    if (widget.drill.length > 0 && onDrill) {
      onDrill(widget.id, field, value, rowObj);
    }
  };

  switch (widget.widget_kind) {
    case "KPI_CARD":
      return (
        <KpiWidget
          widget={widget}
          data={data}
          onItemClick={
            crossFilter || widget.drill.length > 0 ? handleItemClick : undefined
          }
        />
      );
    case "TABLE":
      return (
        <TableWidget
          widget={widget}
          data={data}
          onRowClick={(rowObj) => {
            const nodeIdVal = rowObj.node_id ?? rowObj.id;
            const nodeTypeVal = rowObj.type_name;
            if (
              typeof nodeIdVal === "string" &&
              typeof nodeTypeVal === "string" &&
              onSelectNode
            ) {
              onSelectNode({
                nodeId: nodeIdVal,
                typeName: nodeTypeVal,
                nodeKey:
                  typeof rowObj.node_key === "string"
                    ? rowObj.node_key
                    : undefined,
                title:
                  typeof rowObj.title === "string" ? rowObj.title : undefined,
              });
              return;
            }
            const firstCol = data?.columns[0]?.name;
            if (firstCol && rowObj[firstCol] !== undefined) {
              handleItemClick(firstCol, rowObj[firstCol], rowObj);
            }
          }}
        />
      );
    case "PIVOT_TABLE":
      return (
        <PivotTableWidget
          widget={widget}
          data={data}
          onCellClick={
            crossFilter || widget.drill.length > 0 ? handleItemClick : undefined
          }
        />
      );
    case "LINE_CHART":
    case "BAR_CHART":
    case "PIE_CHART":
    case "SCATTER":
    case "SANKEY":
    case "HEATMAP":
    case "GAUGE":
    case "RADAR":
    case "GRAPH_TOPOLOGY":
      return (
        <EChartsWidget
          widget={widget}
          data={data}
          isDark={isDark}
          nodeLabelMap={nodeLabelMap}
          onDataPointClick={
            widget.widget_kind === "GRAPH_TOPOLOGY"
              ? (_field, val) => {
                  if (typeof val === "string" && onSelectNode) {
                    onSelectNode({ nodeId: val });
                  }
                }
              : handleItemClick
          }
        />
      );
    case "MAP":
      return (
        <MapWidget
          widget={widget}
          data={data}
          isDark={isDark}
          onSelectNode={onSelectNode}
          onCrossFilter={(field, val) => {
            if (crossFilter && onCrossFilter) {
              onCrossFilter(crossFilter.target_filter_key, val);
            } else if (onCrossFilter && field) {
              onCrossFilter(field, val);
            }
          }}
        />
      );
    case "FLOOR_PLAN":
      return (
        <FloorPlanWidget
          widget={widget}
          data={data}
          isDark={isDark}
          editable={isEditing && !readOnly}
          onSelectNode={onSelectNode}
          onCrossFilter={(_field, val) => {
            if (crossFilter && onCrossFilter) {
              onCrossFilter(crossFilter.target_filter_key, val);
            }
          }}
        />
      );
    case "SCENE_3D":
      return (
        <Scene3dWidget
          widget={widget}
          data={data}
          isDark={isDark}
          onSelectNode={onSelectNode}
          onCrossFilter={(_field, val) => {
            if (crossFilter && onCrossFilter) {
              onCrossFilter(crossFilter.target_filter_key, val);
            }
          }}
        />
      );
    case "ALERT_LIST":
      return (
        <AlertListWidget
          widget={widget}
          data={data}
          readOnly={readOnly}
          onAckSuccess={onActionSuccess}
          onSelectNode={(nodeId, title) => onSelectNode?.({ nodeId, title })}
        />
      );
    case "SIM_COMPARE":
      return <SimCompareWidget widget={widget} data={data} isDark={isDark} />;
    case "ACTION_BUTTON":
    case "ACTION_FORM":
      return (
        <ActionWidget
          widget={widget}
          data={data}
          readOnly={readOnly}
          selectedNodeId={selectedNodeId}
          filterValues={filterValues}
          knownNodes={knownNodes}
          onActionSuccess={onActionSuccess}
        />
      );
    case "MARKDOWN_NOTE":
      return <MarkdownWidget widget={widget} data={data} />;
  }
}
