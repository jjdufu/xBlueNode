import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { useI18n } from "@/i18n";
import { xblueApi, type GeoShapeDto } from "@/lib/api/xblue";
import {
  xblueviewApi,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { useTheme } from "@/lib/theme";
import { buildColMap, parseNodesGeoData, readCellByName, type GeoNodePoint } from "./logic";
import {
  parseThresholds,
  readString,
  resolveThresholdColor,
  type SelectedNodeContext,
} from "./utils";

export interface MapWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onCrossFilter?: (field: string, value: unknown) => void;
}

interface PolygonOverlayItem {
  id: string;
  label: string;
  kind: "geofence" | "hull";
  ring: Array<[number, number]>;
}

function isWebGlSupported(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const canvas = document.createElement("canvas");
    return Boolean(canvas.getContext("webgl2") || canvas.getContext("webgl"));
  } catch {
    return false;
  }
}

function parseRingFromShape(raw: unknown): Array<[number, number]> | null {
  let obj: unknown = raw;
  if (typeof raw === "string" && raw.trim().startsWith("{")) {
    try {
      obj = JSON.parse(raw);
    } catch {
      return null;
    }
  }
  if (
    typeof obj !== "object" ||
    obj === null ||
    (obj as { type?: unknown }).type !== "Polygon"
  ) {
    return null;
  }
  const coords = (obj as { coordinates?: unknown }).coordinates;
  if (!Array.isArray(coords) || !Array.isArray(coords[0])) return null;
  const ring: Array<[number, number]> = [];
  for (const pair of coords[0]) {
    if (
      Array.isArray(pair) &&
      pair.length >= 2 &&
      typeof pair[0] === "number" &&
      typeof pair[1] === "number" &&
      Number.isFinite(pair[0]) &&
      Number.isFinite(pair[1])
    ) {
      ring.push([pair[0], pair[1]]);
    }
  }
  return ring.length >= 4 ? ring : null;
}

function extractGeofencePolygons(
  options: Record<string, unknown> | undefined,
  data: WidgetDataResponse | null
): PolygonOverlayItem[] {
  const overlays: PolygonOverlayItem[] = [];
  if (options?.geofence_polygon) {
    const ring = parseRingFromShape(options.geofence_polygon);
    if (ring) {
      overlays.push({ id: "opt-geofence", label: "Geofence", kind: "geofence", ring });
    }
  }
  if (Array.isArray(options?.polygons)) {
    options.polygons.forEach((item, idx) => {
      const ring = parseRingFromShape(item);
      if (ring) {
        overlays.push({
          id: `opt-poly-${idx}`,
          label: `Polygon ${idx + 1}`,
          kind: "geofence",
          ring,
        });
      }
    });
  }
  if (data && data.columns.length > 0) {
    const colMap = buildColMap(data);
    if (colMap.has("geo_shape")) {
      data.rows.forEach((row, idx) => {
        const shapeCell = readCellByName(row, colMap, "geo_shape");
        const ring = parseRingFromShape(shapeCell);
        if (ring) {
          const titleCell = readCellByName(row, colMap, "title");
          overlays.push({
            id: `row-poly-${idx}`,
            label: typeof titleCell === "string" ? titleCell : `Zone ${idx + 1}`,
            kind: "geofence",
            ring,
          });
        }
      });
    }
  }
  return overlays;
}

function fallbackPlanarConvexHull(
  pts: ReadonlyArray<{ lng: number; lat: number }>
): Array<[number, number]> | null {
  if (pts.length < 3) return null;
  const sorted = [...pts].sort((a, b) =>
    a.lng !== b.lng ? a.lng - b.lng : a.lat - b.lat
  );
  const cross = (
    o: { lng: number; lat: number },
    a: { lng: number; lat: number },
    b: { lng: number; lat: number }
  ) => (a.lng - o.lng) * (b.lat - o.lat) - (a.lat - o.lat) * (b.lng - o.lng);

  const lower: Array<{ lng: number; lat: number }> = [];
  for (const p of sorted) {
    while (
      lower.length >= 2 &&
      cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0
    ) {
      lower.pop();
    }
    lower.push(p);
  }
  const upper: Array<{ lng: number; lat: number }> = [];
  for (let i = sorted.length - 1; i >= 0; i -= 1) {
    const p = sorted[i];
    while (
      upper.length >= 2 &&
      cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0
    ) {
      upper.pop();
    }
    upper.push(p);
  }
  lower.pop();
  upper.pop();
  const hull = [...lower, ...upper];
  if (hull.length < 3) return null;
  const ring: Array<[number, number]> = hull.map((p) => [p.lng, p.lat]);
  ring.push([hull[0].lng, hull[0].lat]);
  return ring;
}

export function MapWidget({
  widget,
  data,
  isDark,
  onSelectNode,
  onCrossFilter,
}: MapWidgetProps) {
  const { tx, fmtNum } = useI18n();
  const { isDark: themeDark } = useTheme();
  const effectiveDark = isDark ?? themeDark;

  const echartsContainerRef = useRef<HTMLDivElement | null>(null);
  const echartsRef = useRef<echarts.ECharts | null>(null);
  const maplibreContainerRef = useRef<HTMLDivElement | null>(null);
  const maplibreRef = useRef<maplibregl.Map | null>(null);

  const [geoJsonReady, setGeoJsonReady] = useState<string | null>(null);
  const [showConvexHull, setShowConvexHull] = useState<boolean>(() =>
    Boolean(widget.options?.show_convex_hull)
  );
  const [hullShape, setHullShape] = useState<GeoShapeDto | null>(null);

  const envTileUrl =
    import.meta.env.AXPOINT_MAP_TILE_URL || import.meta.env.VITE_MAP_TILE_URL;
  const tileUrl =
    readString(widget.options, "tile_url") ??
    (envTileUrl && envTileUrl.trim().length > 0 ? envTileUrl.trim() : undefined);
  const geojsonAssetId = readString(widget.options, "geojson_asset_id");
  const useMaplibre = Boolean(tileUrl && isWebGlSupported());

  const thresholds = useMemo(
    () => parseThresholds(widget.options),
    [widget.options]
  );

  const { points, flyLines } = useMemo(
    () => parseNodesGeoData(data),
    [data]
  );

  const geofences = useMemo(
    () => extractGeofencePolygons(widget.options, data),
    [data, widget.options]
  );

  useEffect(() => {
    if (!showConvexHull || points.length < 3) {
      setHullShape(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const resp = await xblueApi.computeConvexHull({
          points: points.map((p) => ({ lng: p.lng, lat: p.lat })),
        });
        if (!cancelled && resp.hull?.type === "Polygon") {
          setHullShape(resp.hull);
          return;
        }
      } catch {
        // Fallback to deterministic local planar convex hull when offline
      }
      if (!cancelled) {
        const ring = fallbackPlanarConvexHull(points);
        setHullShape(ring ? { type: "Polygon", coordinates: [ring] } : null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [points, showConvexHull]);

  const polygonOverlays = useMemo<PolygonOverlayItem[]>(() => {
    const merged = [...geofences];
    if (hullShape?.type === "Polygon") {
      const ring = parseRingFromShape(hullShape);
      if (ring) {
        merged.push({
          id: "cluster-convex-hull",
          label: tx("凸包", "Hull"),
          kind: "hull",
          ring,
        });
      }
    }
    return merged;
  }, [geofences, hullShape, tx]);

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
        echarts.registerMap(
          mapName,
          raw as Parameters<typeof echarts.registerMap>[1]
        );
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
    const sources: Record<string, maplibregl.SourceSpecification> = {
      osm: {
        type: "raster",
        tiles: [tileUrl],
        tileSize: 256,
      },
    };
    const layers: maplibregl.LayerSpecification[] = [
      { id: "osm", type: "raster", source: "osm" },
    ];

    if (polygonOverlays.length > 0) {
      sources["xv-polygons"] = {
        type: "geojson",
        data: {
          type: "FeatureCollection",
          features: polygonOverlays.map((poly, idx) => ({
            type: "Feature" as const,
            id: idx,
            properties: { label: poly.label, kind: poly.kind },
            geometry: {
              type: "Polygon" as const,
              coordinates: [poly.ring],
            },
          })),
        },
      };
      layers.push(
        {
          id: "xv-polygons-fill",
          type: "fill",
          source: "xv-polygons",
          paint: {
            "fill-color": effectiveDark ? "#10b981" : "#059669",
            "fill-opacity": 0.14,
          },
        },
        {
          id: "xv-polygons-outline",
          type: "line",
          source: "xv-polygons",
          paint: {
            "line-color": effectiveDark ? "#34d399" : "#059669",
            "line-width": 1.8,
            "line-dasharray": [3, 2],
          },
        }
      );
    }

    if (flyLines.length > 0) {
      sources["xv-fly-lines"] = {
        type: "geojson",
        data: {
          type: "FeatureCollection",
          features: flyLines.map((lk, idx) => ({
            type: "Feature" as const,
            id: idx,
            properties: {
              link_type: lk.linkType,
              weight: lk.weight,
            },
            geometry: {
              type: "LineString" as const,
              coordinates: [
                [lk.srcLng, lk.srcLat],
                [lk.dstLng, lk.dstLat],
              ],
            },
          })),
        },
      };
      layers.push({
        id: "xv-fly-lines-layer",
        type: "line",
        source: "xv-fly-lines",
        paint: {
          "line-color": effectiveDark ? "#3b82f6" : "#2563eb",
          "line-width": 2,
          "line-opacity": 0.85,
        },
      });
    }

    const map = new maplibregl.Map({
      container: el,
      style: {
        version: 8,
        sources,
        layers,
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
  }, [
    effectiveDark,
    flyLines,
    points,
    polygonOverlays,
    thresholds,
    tileUrl,
    useMaplibre,
  ]);

  // ECharts Geo / Cartesian mode
  useEffect(() => {
    if (useMaplibre) return;
    const el = echartsContainerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 180,
    });
    echartsRef.current = chart;

    chart.on("click", (params) => {
      if (typeof params.data === "object" && params.data !== null) {
        const rec = params.data as { node?: GeoNodePoint };
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

    const ro = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
      }
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      echartsRef.current = null;
      try {
        if (!chart.isDisposed()) {
          chart.dispose();
        }
      } catch {
        // Ignore ZRender motionBlur 0x0 back-buffer canvas InvalidStateError on unmount
      }
    };
  }, [useMaplibre]);

  useEffect(() => {
    const chart = echartsRef.current;
    if (!chart || chart.isDisposed() || useMaplibre) return;

    const fg = effectiveDark ? "#fafafa" : "#09090b";
    const muted = effectiveDark ? "#a1a1aa" : "#71717a";
    const gridLine = effectiveDark ? "#27272a" : "#e4e4e7";
    const cardBg = effectiveDark ? "#18181b" : "#ffffff";
    const lineAccent = effectiveDark ? "#3b82f6" : "#2563eb";
    const polyStroke = effectiveDark ? "#34d399" : "#059669";

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

    const linesSeriesData = flyLines.map((lk) => ({
      name: lk.linkType,
      value: lk.weight,
      coords: [
        [lk.srcLng, lk.srcLat],
        [lk.dstLng, lk.dstLat],
      ] as [[number, number], [number, number]],
    }));

    if (geoJsonReady) {
      const geoSeries: echarts.SeriesOption[] = [
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
          labelLayout: {
            hideOverlap: true,
            moveOverlap: "shiftY",
          },
        },
      ];
      if (polygonOverlays.length > 0) {
        geoSeries.push({
          type: "lines",
          coordinateSystem: "geo",
          polyline: true,
          lineStyle: {
            color: polyStroke,
            width: 1.8,
            type: "dashed",
            opacity: 0.85,
          },
          data: polygonOverlays.map((poly) => ({
            name: poly.label,
            coords: poly.ring,
          })),
        });
      }
      if (linesSeriesData.length > 0) {
        geoSeries.push({
          type: "lines",
          coordinateSystem: "geo",
          zlevel: 1,
          effect: { show: true, period: 4, trailLength: 0.2, symbolSize: 5 },
          lineStyle: {
            color: lineAccent,
            width: 1.6,
            opacity: 0.75,
            curveness: 0.2,
          },
          data: linesSeriesData,
        });
      }
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
              areaColor: effectiveDark ? "#18181b" : "#f4f4f5",
              borderColor: gridLine,
            },
            emphasis: {
              itemStyle: {
                areaColor: effectiveDark ? "#27272a" : "#e4e4e7",
              },
            },
          },
          series: geoSeries,
        },
        true
      );
      return;
    }

    const polyCoords = polygonOverlays.flatMap((p) => p.ring);
    const lngs = [
      ...points.map((p) => p.lng),
      ...flyLines.flatMap((lk) => [lk.srcLng, lk.dstLng]),
      ...polyCoords.map((c) => c[0]),
    ];
    const lats = [
      ...points.map((p) => p.lat),
      ...flyLines.flatMap((lk) => [lk.srcLat, lk.dstLat]),
      ...polyCoords.map((c) => c[1]),
    ];
    const rawMinLng = lngs.length > 0 ? Math.min(...lngs) : 100;
    const rawMaxLng = lngs.length > 0 ? Math.max(...lngs) : 130;
    const rawMinLat = lats.length > 0 ? Math.min(...lats) : 20;
    const rawMaxLat = lats.length > 0 ? Math.max(...lats) : 45;
    const lngSpan = Math.max(0.8, rawMaxLng - rawMinLng);
    const latSpan = Math.max(0.8, rawMaxLat - rawMinLat);
    const minLng = Number((rawMinLng - lngSpan * 0.16).toFixed(2));
    const maxLng = Number((rawMaxLng + lngSpan * 0.26).toFixed(2));
    const minLat = Number((rawMinLat - latSpan * 0.18).toFixed(2));
    const maxLat = Number((rawMaxLat + latSpan * 0.18).toFixed(2));

    const cartesianSeries: echarts.SeriesOption[] = [];
    for (const poly of polygonOverlays) {
      cartesianSeries.push({
        type: "line",
        name: poly.label,
        data: poly.ring,
        showSymbol: false,
        lineStyle: {
          color: poly.kind === "hull" ? lineAccent : polyStroke,
          width: 1.6,
          type: poly.kind === "hull" ? "dashed" : "solid",
        },
        areaStyle: {
          color: poly.kind === "hull" ? lineAccent : polyStroke,
          opacity: 0.1,
        },
      });
    }

    cartesianSeries.push({
      type: "effectScatter",
      symbolSize: 13,
      data: seriesData,
      label: {
        show: seriesData.length <= 30,
        formatter: "{b}",
        position: "right",
        distance: 5,
        color: fg,
        fontSize: 10.5,
      },
      labelLayout: {
        hideOverlap: true,
        moveOverlap: "shiftY",
      },
    });

    if (linesSeriesData.length > 0) {
      cartesianSeries.push({
        type: "lines",
        coordinateSystem: "cartesian2d",
        zlevel: 1,
        effect: { show: true, period: 4, trailLength: 0.2, symbolSize: 5 },
        lineStyle: {
          color: lineAccent,
          width: 1.6,
          opacity: 0.75,
          curveness: 0.2,
        },
        data: linesSeriesData,
      });
    }

    chart.setOption(
      {
        animation: false,
        tooltip: {
          trigger: "item",
          backgroundColor: cardBg,
          borderColor: gridLine,
          textStyle: { color: fg, fontSize: 12 },
          formatter: (params: unknown) => {
            const p = params as {
              data?: { node?: GeoNodePoint; name?: string; value?: unknown };
            };
            const node = p.data?.node;
            if (!node) {
              return p.data?.name ? `${p.data.name}` : "";
            }
            return `${node.title} (${node.nodeKey})<br/>${tx("坐标", "Coords")}: ${node.lng.toFixed(3)}, ${node.lat.toFixed(3)}${
              node.metricValue !== null
                ? `<br/>${tx("指标", "Value")}: ${fmtNum(node.metricValue, 2)}`
                : ""
            }`;
          },
        },
        grid: {
          top: 28,
          right: 36,
          bottom: 28,
          left: 48,
          containLabel: true,
        },
        xAxis: {
          type: "value",
          name: tx("经度", "Lng"),
          nameLocation: "end",
          nameGap: 5,
          nameTextStyle: { color: muted, fontSize: 10 },
          min: minLng,
          max: maxLng,
          splitNumber: 4,
          axisLabel: { color: muted, fontSize: 10, hideOverlap: true },
          axisLine: { lineStyle: { color: gridLine } },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        yAxis: {
          type: "value",
          name: tx("纬度", "Lat"),
          nameLocation: "end",
          nameGap: 8,
          nameTextStyle: { color: muted, fontSize: 10, align: "left" },
          min: minLat,
          max: maxLat,
          splitNumber: 4,
          axisLabel: { color: muted, fontSize: 10, hideOverlap: true },
          axisLine: { lineStyle: { color: gridLine } },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        series: cartesianSeries,
      },
      true
    );
  }, [
    effectiveDark,
    flyLines,
    fmtNum,
    geoJsonReady,
    points,
    polygonOverlays,
    thresholds,
    tx,
    useMaplibre,
  ]);

  return (
    <div className="h-full relative" data-testid={`widget-map-${widget.id}`}>
      {points.length >= 3 && (
        <div className="absolute top-1.5 right-2 z-10">
          <button
            type="button"
            onClick={() => setShowConvexHull((prev) => !prev)}
            className={`rounded border px-2 py-0.5 text-[11px] cursor-pointer ${
              showConvexHull
                ? "border-xblue bg-xblue-soft text-xblue font-medium"
                : "border-border bg-background/90 text-muted-foreground hover:text-foreground"
            }`}
            data-testid="map-convex-hull-toggle"
          >
            {tx("凸包", "Hull")}
          </button>
        </div>
      )}
      {useMaplibre ? (
        <div
          ref={maplibreContainerRef}
          className="h-full w-full min-h-[160px] rounded overflow-hidden"
        />
      ) : (
        <div ref={echartsContainerRef} className="h-full w-full min-h-[160px]" />
      )}
      {points.length === 0 && flyLines.length === 0 && polygonOverlays.length === 0 && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none text-xs text-muted-foreground">
          {tx("暂无地理坐标节点", "No geo-located nodes")}
        </div>
      )}
    </div>
  );
}
