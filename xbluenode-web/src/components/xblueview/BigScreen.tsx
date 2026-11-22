import { useEffect, useRef, useState } from "react";
import { ArrowLeft, Maximize2, Minimize2, Radio } from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  DashboardDetail,
  DrillPathItem,
  WidgetDataResponse,
} from "@/lib/api/xblueview";
import { DashboardGrid } from "./DashboardGrid";
import type { SelectedNodeContext } from "./utils";

export interface BigScreenProps {
  dashboard: DashboardDetail;
  widgetDataMap: Record<string, WidgetDataResponse | null>;
  loadingMap: Record<string, boolean>;
  errorMap: Record<string, string | null>;
  drillMap: Record<string, DrillPathItem[]>;
  sseConnected: boolean;
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  onExitScreen: () => void;
  onRefreshWidget: (widgetId: string) => void;
  onDrill: (
    widgetId: string,
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
  onPopDrill: (widgetId: string, level: number) => void;
  onCrossFilter: (filterKey: string, value: unknown) => void;
  onSelectNode: (node: SelectedNodeContext) => void;
  onActionSuccess: () => void;
}

export function BigScreen({
  dashboard,
  widgetDataMap,
  loadingMap,
  errorMap,
  drillMap,
  sseConnected,
  selectedNodeId,
  filterValues,
  onExitScreen,
  onRefreshWidget,
  onDrill,
  onPopDrill,
  onCrossFilter,
  onSelectNode,
  onActionSuccess,
}: BigScreenProps) {
  const { tx } = useI18n();
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [viewport, setViewport] = useState(() => ({
    w: typeof window !== "undefined" ? window.innerWidth : 1920,
    h: typeof window !== "undefined" ? window.innerHeight : 1080,
  }));

  const designW =
    dashboard.canvas.screen_width > 0 ? dashboard.canvas.screen_width : 1920;
  const designH =
    dashboard.canvas.screen_height > 0 ? dashboard.canvas.screen_height : 1080;

  useEffect(() => {
    const onResize = () => {
      setViewport({
        w: window.innerWidth,
        h: window.innerHeight,
      });
    };
    const onFsChange = () => {
      setIsFullscreen(Boolean(document.fullscreenElement));
    };
    window.addEventListener("resize", onResize);
    document.addEventListener("fullscreenchange", onFsChange);
    return () => {
      window.removeEventListener("resize", onResize);
      document.removeEventListener("fullscreenchange", onFsChange);
    };
  }, []);

  const scale = Math.min(viewport.w / designW, viewport.h / designH);
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;

  const toggleFullscreen = async () => {
    try {
      if (!document.fullscreenElement) {
        await (
          rootRef.current ?? document.documentElement
        ).requestFullscreen();
      } else {
        await document.exitFullscreen();
      }
    } catch {
      // Ignore fullscreen denial in restricted browsers
    }
  };

  return (
    <div
      ref={rootRef}
      className="dark fixed inset-0 z-40 bg-zinc-950 text-zinc-100 overflow-hidden flex items-center justify-center"
      data-testid="big-screen-container"
    >
      <div
        data-testid="big-screen-stage"
        style={{
          width: `${designW}px`,
          height: `${designH}px`,
          transform: `scale(${safeScale})`,
          transformOrigin: "center center",
        }}
        className="shrink-0 flex flex-col bg-zinc-950 border border-zinc-800/80 shadow-2xl overflow-hidden"
      >
        {/* Top Screen Header */}
        <header className="px-6 py-3 border-b border-zinc-800 flex items-center justify-between bg-zinc-900/70 shrink-0">
          <div className="flex items-center gap-3">
            <span className="h-2.5 w-2.5 rounded-full bg-xblue" />
            <h1 className="text-xl font-semibold tracking-tight">
              {dashboard.name}
            </h1>
            {dashboard.description && (
              <span className="text-xs text-zinc-400">
                {dashboard.description}
              </span>
            )}
          </div>

          <div className="flex items-center gap-3 text-xs">
            <span className="inline-flex items-center gap-1.5 rounded border border-zinc-800 bg-zinc-900 px-2.5 py-1 font-mono-tech text-zinc-300">
              <Radio
                className={`h-3 w-3 ${
                  sseConnected ? "text-xblue animate-pulse" : "text-zinc-500"
                }`}
              />
              {sseConnected ? tx("实时流", "Live") : tx("轮询", "Polling")}
            </span>
            <span className="font-mono-tech text-zinc-400">
              {designW}×{designH} ({Math.round(safeScale * 100)}%)
            </span>
            <button
              type="button"
              onClick={() => void toggleFullscreen()}
              className="inline-flex items-center gap-1 rounded border border-zinc-700 px-2.5 py-1 hover:bg-zinc-800 cursor-pointer"
              data-testid="big-screen-fullscreen-btn"
            >
              {isFullscreen ? (
                <Minimize2 className="h-3.5 w-3.5" />
              ) : (
                <Maximize2 className="h-3.5 w-3.5" />
              )}
              {isFullscreen
                ? tx("退出全屏", "Exit Fullscreen")
                : tx("全屏", "Fullscreen")}
            </button>
            <button
              type="button"
              onClick={onExitScreen}
              className="inline-flex items-center gap-1 rounded border border-zinc-700 px-2.5 py-1 hover:bg-zinc-800 cursor-pointer"
              data-testid="big-screen-exit-btn"
            >
              <ArrowLeft className="h-3.5 w-3.5" />
              {tx("退出大屏", "Exit Screen")}
            </button>
          </div>
        </header>

        {/* Scaled Grid Content */}
        <div className="flex-1 overflow-y-auto p-5">
          <DashboardGrid
            widgets={dashboard.widgets}
            canvas={dashboard.canvas}
            widgetDataMap={widgetDataMap}
            loadingMap={loadingMap}
            errorMap={errorMap}
            drillMap={drillMap}
            isEditing={false}
            isDark={true}
            isScreen={true}
            fixedWidth={designW - 40}
            selectedNodeId={selectedNodeId}
            filterValues={filterValues}
            onRefreshWidget={onRefreshWidget}
            onDrill={onDrill}
            onPopDrill={onPopDrill}
            onCrossFilter={onCrossFilter}
            onSelectNode={onSelectNode}
            onActionSuccess={onActionSuccess}
          />
        </div>
      </div>
    </div>
  );
}
