import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import {
  xblueviewApi,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import {
  buildColumnIndexMap,
  getCell,
  parseThresholds,
  readString,
  resolveThresholdColor,
  toNumericValue,
  type SelectedNodeContext,
} from "./utils";

export interface MapWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onCrossFilter?: (field: string, value: unknown) => void;
}

interface GeoNodeItem {
  nodeId: string;
  typeName: string;
  nodeKey: string;
  title: string;
  lng: number;
  lat: number;
  metricValue: number | null;
}

function isWebGlSupported(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const canvas = document.createElement("canvas");
    return Boolean(
      canvas.getContext("webgl2") || canvas.getContext("webgl")
    );
  } catch {
    return false;
  }
}

export function MapWidget({
  widget,
  data,
  isDark = false,
  onSelectNode,
  onCrossFilter,
}: MapWidgetProps) {
  const echartsContainerRef = useRef<HTMLDivElement | null>(null);
  const echartsRef = useRef<echarts.ECharts | null>(null);
  const maplibreContainerRef = useRef<HTMLDivElement | null>(null);
  const maplibreRef = useRef<maplibregl.Map | null>(null);

  const [geoJsonReady, setGeoJsonReady] = useState<string | null>(null);

  const tileUrl =
    readString(widget.options, "tile_url") ??
    (import.meta.env.VITE_MAP_TILE_URL as string | undefined);
  const geojsonAssetId = readString(widget.options, "geojson_asset_id");
  const useMaplibre = Boolean(tileUrl && isWebGlSupported());

  const thresholds = useMemo(
    () => parseThresholds(widget.options),
    [widget.options]
  );

  const points = useMemo<GeoNodeItem[]>(() => {
    if (!data) return [];
    const colMap = buildColumnIndexMap(data);
    const items: GeoNodeItem[] = [];
    for (const row of data.rows) {
      const lng = toNumericValue(
        getCell(row, colMap, "geo_lng", -1) ??
          getCell(row, colMap, "lng", 4)
      );
      const lat = toNumericValue(
        getCell(row, colMap, "geo_lat", -1) ??
          getCell(row, colMap, "lat", 5)
      );
      if (lng === null || lat === null) continue;
      const nodeId = String(
        getCell(row, colMap, "node_id", 0) ??
          getCell(row, colMap, "id", 0) ??
          ""
      );
      const typeName = String(getCell(row, colMap, "type_name", 1) ?? "Node");
      const nodeKey = String(getCell(row, colMap, "node_key", 2) ?? nodeId);
      const title = String(getCell(row, colMap, "title", 3) ?? nodeKey);
      const metricValue = toNumericValue(
        getCell(row, colMap, "metric_value", 6) ??
          getCell(row, colMap, "value", 6)
      );
      items.push({
        nodeId,
        typeName,
        nodeKey,
        title,
        lng,
        lat,
        metricValue,
      });
    }
    return items;
  }, [data]);

  // Load optional GEOJSON asset
  useEffect(() => {
    if (!geojsonAssetId || useMaplibre) {
      setGeoJsonReady(null);
      return;
    }
    let cancelled = false;
    let blobUrl: string | null = null;
    const loadGeo = async () => {
      try {
        blobUrl = await xblueviewApi.fetchAssetBlobUrl(geojsonAssetId);
        const resp = await fetch(blobUrl);
        const raw: unknown = await resp.json();
        if (cancelled) return;
        const mapName = `xv_geo_${geojsonAssetId}`;
        echarts.registerMap(mapName, raw as Parameters<typeof echarts.registerMap>[1]);
        setGeoJsonReady(mapName);
      } catch {
        if (!cancelled) setGeoJsonReady(null);
      } finally {
        if (blobUrl) URL.revokeObjectURL(blobUrl);
      }
    };
    void loadGeo();
    return () => {
      cancelled = true;
    };
  }, [geojsonAssetId, useMaplibre]);

  const selectRef = useRef(onSelectNode);
  const crossRef = useRef(onCrossFilter);
  useEffect(() => {
    selectRef.current = onSelectNode;
    crossRef.current = onCrossFilter;
  }, [onCrossFilter, onSelectNode]);

  // MapLibre mode when tile URL is configured and WebGL works
  useEffect(() => {
    if (!useMaplibre || !tileUrl) return;
    const el = maplibreContainerRef.current;
    if (!el) return;

    const first = points[0];
    const map = new maplibregl.Map({
      container: el,
      style: {
        version: 8,
        sources: {
          osm: {
            type: "raster",
            tiles: [tileUrl],
            tileSize: 256,
          },
        },
        layers: [{ id: "osm", type: "raster", source: "osm" }],
      },
      center: first ? [first.lng, first.lat] : [121.47, 31.23],
      zoom: 6,
    });
    maplibreRef.current = map;

    const markers: maplibregl.Marker[] = [];
    for (const pt of points) {
      const color = resolveThresholdColor(pt.metricValue, thresholds, "#2563eb");
      const markerEl = document.createElement("button");
      markerEl.type = "button";
      markerEl.title = `${pt.title}${pt.metricValue !== null ? `: ${pt.metricValue}` : ""}`;
      markerEl.style.width = "14px";
      markerEl.style.height = "14px";
      markerEl.style.borderRadius = "9999px";
      markerEl.style.backgroundColor = color;
      markerEl.style.border = "2px solid #ffffff";
      markerEl.style.cursor = "pointer";
      markerEl.onclick = () => {
        if (pt.nodeId && selectRef.current) {
          selectRef.current({
            nodeId: pt.nodeId,
            typeName: pt.typeName,
            nodeKey: pt.nodeKey,
            title: pt.title,
            metricValue: pt.metricValue,
          });
        }
        crossRef.current?.("node_key", pt.nodeKey);
      };
      const marker = new maplibregl.Marker({ element: markerEl })
        .setLngLat([pt.lng, pt.lat])
        .addTo(map);
      markers.push(marker);
    }

    return () => {
      for (const m of markers) m.remove();
      map.remove();
      maplibreRef.current = null;
    };
  }, [points, thresholds, tileUrl, useMaplibre]);

  // ECharts Geo / Cartesian mode
  useEffect(() => {
    if (useMaplibre) return;
    const el = echartsContainerRef.current;
    if (!el) return;
    const chart = echarts.init(el);
    echartsRef.current = chart;

    chart.on("click", (params) => {
      if (typeof params.data === "object" && params.data !== null) {
        const rec = params.data as { node?: GeoNodeItem };
        if (rec.node) {
          if (rec.node.nodeId && selectRef.current) {
            selectRef.current({
              nodeId: rec.node.nodeId,
              typeName: rec.node.typeName,
              nodeKey: rec.node.nodeKey,
              title: rec.node.title,
              metricValue: rec.node.metricValue,
            });
          }
          crossRef.current?.("node_key", rec.node.nodeKey);
        }
      }
    });

    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      chart.dispose();
      echartsRef.current = null;
    };
  }, [useMaplibre]);

  useEffect(() => {
    const chart = echartsRef.current;
    if (!chart || useMaplibre) return;

    const fg = isDark ? "#fafafa" : "#09090b";
    const muted = isDark ? "#a1a1aa" : "#71717a";
    const gridLine = isDark ? "#27272a" : "#e4e4e7";
    const cardBg = isDark ? "#18181b" : "#ffffff";

    const seriesData = points.map((pt) => ({
      name: pt.title,
      value: [pt.lng, pt.lat, pt.metricValue ?? 0],
      node: pt,
      itemStyle: {
        color: resolveThresholdColor(pt.metricValue, thresholds, "#2563eb"),
        borderColor: fg,
        borderWidth: 1.5,
      },
    }));

    if (geoJsonReady) {
      chart.setOption(
        {
          animation: false,
          tooltip: {
            trigger: "item",
            backgroundColor: cardBg,
            borderColor: gridLine,
            textStyle: { color: fg, fontSize: 12 },
          },
          geo: {
            map: geoJsonReady,
            roam: true,
            itemStyle: {
              areaColor: isDark ? "#18181b" : "#f4f4f5",
              borderColor: gridLine,
            },
            emphasis: {
              itemStyle: { areaColor: isDark ? "#27272a" : "#e4e4e7" },
            },
          },
          series: [
            {
              type: "effectScatter",
              coordinateSystem: "geo",
              symbolSize: 13,
              data: seriesData,
              label: {
                show: seriesData.length <= 30,
                formatter: "{b}",
                position: "right",
                color: fg,
                fontSize: 10,
              },
            },
          ],
        },
        true
      );
      return;
    }

    const lngs = points.map((p) => p.lng);
    const lats = points.map((p) => p.lat);
    const minLng = lngs.length > 0 ? Math.min(...lngs) - 0.5 : 100;
    const maxLng = lngs.length > 0 ? Math.max(...lngs) + 0.5 : 130;
    const minLat = lats.length > 0 ? Math.min(...lats) - 0.5 : 20;
    const maxLat = lats.length > 0 ? Math.max(...lats) + 0.5 : 45;

    chart.setOption(
      {
        animation: false,
        tooltip: {
          trigger: "item",
          backgroundColor: cardBg,
          borderColor: gridLine,
          textStyle: { color: fg, fontSize: 12 },
          formatter: (params: unknown) => {
            const p = params as { data?: { node?: GeoNodeItem } };
            const node = p.data?.node;
            if (!node) return "";
            return `${node.title} (${node.nodeKey})<br/>经纬度: ${node.lng.toFixed(3)}, ${node.lat.toFixed(3)}${
              node.metricValue !== null ? `<br/>指标: ${node.metricValue}` : ""
            }`;
          },
        },
        grid: { top: 20, right: 20, bottom: 28, left: 46, containLabel: true },
        xAxis: {
          type: "value",
          name: "经度",
          min: minLng,
          max: maxLng,
          axisLabel: { color: muted, fontSize: 10 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        yAxis: {
          type: "value",
          name: "纬度",
          min: minLat,
          max: maxLat,
          axisLabel: { color: muted, fontSize: 10 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        series: [
          {
            type: "effectScatter",
            symbolSize: 14,
            data: seriesData,
            label: {
              show: seriesData.length <= 30,
              formatter: "{b}",
              position: "right",
              color: fg,
              fontSize: 11,
            },
          },
        ],
      },
      true
    );
  }, [geoJsonReady, isDark, points, thresholds, useMaplibre]);

  return (
    <div className="h-full relative" data-testid={`widget-map-${widget.id}`}>
      {useMaplibre ? (
        <div ref={maplibreContainerRef} className="h-full w-full min-h-[160px] rounded overflow-hidden" />
      ) : (
        <div ref={echartsContainerRef} className="h-full w-full min-h-[160px]" />
      )}
      {points.length === 0 && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none text-xs text-muted-foreground">
          暂无带经纬度坐标的业务节点
        </div>
      )}
    </div>
  );
}
