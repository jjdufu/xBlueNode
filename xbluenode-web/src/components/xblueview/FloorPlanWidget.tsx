import { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, Layers } from "lucide-react";
import {
  xblueviewApi,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import {
  buildColumnIndexMap,
  formatDisplayValue,
  getCell,
  parseThresholds,
  readString,
  resolveThresholdColor,
  toNumericValue,
  type SelectedNodeContext,
} from "./utils";

export interface FloorPlanWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onCrossFilter?: (field: string, value: unknown) => void;
}

interface FloorMarker {
  nodeId: string;
  typeName: string;
  nodeKey: string;
  title: string;
  xRatio: number;
  yRatio: number;
  metricValue: number | null;
  openAlerts: number;
}

export function FloorPlanWidget({
  widget,
  data,
  onSelectNode,
  onCrossFilter,
}: FloorPlanWidgetProps) {
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [loadErr, setLoadErr] = useState(false);
  const blobUrlRef = useRef<string | null>(null);

  const assetId =
    widget.data_source.kind === "FLOOR_PLAN"
      ? widget.data_source.asset_id
      : readString(widget.options, "asset_id") ?? "";

  useEffect(() => {
    if (!assetId) {
      setImageUrl(null);
      return;
    }
    let cancelled = false;
    setLoadErr(false);
    xblueviewApi
      .fetchAssetBlobUrl(assetId)
      .then((url) => {
        if (cancelled) {
          URL.revokeObjectURL(url);
          return;
        }
        if (blobUrlRef.current) {
          URL.revokeObjectURL(blobUrlRef.current);
        }
        blobUrlRef.current = url;
        setImageUrl(url);
      })
      .catch(() => {
        if (!cancelled) {
          setLoadErr(true);
          setImageUrl(null);
        }
      });

    return () => {
      cancelled = true;
      if (blobUrlRef.current) {
        URL.revokeObjectURL(blobUrlRef.current);
        blobUrlRef.current = null;
      }
    };
  }, [assetId]);

  const thresholds = useMemo(
    () => parseThresholds(widget.options),
    [widget.options]
  );

  const markers = useMemo<FloorMarker[]>(() => {
    if (!data) return [];
    const colMap = buildColumnIndexMap(data);
    return data.rows.map((r, idx) => {
      const nodeId = String(getCell(r, colMap, "node_id", 0) ?? `node-${idx}`);
      const typeName = String(getCell(r, colMap, "type_name", 1) ?? "Node");
      const nodeKey = String(getCell(r, colMap, "node_key", 2) ?? nodeId);
      const title = String(getCell(r, colMap, "title", 3) ?? nodeKey);
      const xRatio = toNumericValue(getCell(r, colMap, "x_ratio", 4)) ?? 0.5;
      const yRatio = toNumericValue(getCell(r, colMap, "y_ratio", 5)) ?? 0.5;
      const metricValue = toNumericValue(getCell(r, colMap, "metric_value", 6));
      const openAlerts = toNumericValue(getCell(r, colMap, "open_alerts", 7)) ?? 0;
      return {
        nodeId,
        typeName,
        nodeKey,
        title,
        xRatio: Math.max(0.02, Math.min(0.98, xRatio)),
        yRatio: Math.max(0.02, Math.min(0.98, yRatio)),
        metricValue,
        openAlerts,
      };
    });
  }, [data]);

  return (
    <div
      className="h-full relative rounded border border-border bg-muted/30 overflow-hidden select-none"
      data-testid={`widget-floorplan-${widget.id}`}
    >
      {imageUrl && !loadErr ? (
        <img
          src={imageUrl}
          alt={widget.title || "平面图"}
          className="w-full h-full object-contain pointer-events-none"
        />
      ) : (
        <div
          className="w-full h-full flex flex-col items-center justify-center text-xs text-muted-foreground"
          style={{
            backgroundImage:
              "radial-gradient(circle, currentColor 1px, transparent 1px)",
            backgroundSize: "20px 20px",
          }}
        >
          <Layers className="h-5 w-5 mb-1 opacity-50" />
          <span>{assetId ? "平面图底图坐标视图" : "未绑定平面图资产"}</span>
        </div>
      )}

      {markers.map((m) => {
        const color = resolveThresholdColor(m.metricValue, thresholds, "#2563eb");
        const hasAlert = m.openAlerts > 0;
        return (
          <button
            key={m.nodeId}
            type="button"
            onClick={() => {
              onSelectNode?.({
                nodeId: m.nodeId,
                typeName: m.typeName,
                nodeKey: m.nodeKey,
                title: m.title,
                metricValue: m.metricValue,
              });
              onCrossFilter?.("node_key", m.nodeKey);
            }}
            style={{
              left: `${m.xRatio * 100}%`,
              top: `${m.yRatio * 100}%`,
              borderColor: color,
            }}
            className={`absolute -translate-x-1/2 -translate-y-1/2 rounded-md border-2 bg-background/95 px-2 py-1 text-[11px] shadow-sm transition-transform hover:scale-105 cursor-pointer ${
              hasAlert ? "animate-pulse ring-2 ring-xblue" : ""
            }`}
          >
            <div className="flex items-center gap-1 font-medium">
              <span
                className="h-2 w-2 rounded-full shrink-0"
                style={{ backgroundColor: color }}
              />
              <span className="truncate max-w-[96px]">{m.title}</span>
              {hasAlert && <AlertTriangle className="h-3 w-3 text-xblue shrink-0" />}
            </div>
            {m.metricValue !== null && (
              <div className="font-mono-tech text-[10px] text-muted-foreground">
                {formatDisplayValue(m.metricValue)}
              </div>
            )}
          </button>
        );
      })}
    </div>
  );
}
