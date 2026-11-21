import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import {
  ArrowLeft,
  CheckCircle2,
  Copy,
  Edit3,
  FolderKanban,
  Layers,
  MessageSquare,
  Monitor,
  Plus,
  Radio,
  RefreshCw,
  Save,
  Search,
  Settings2,
  Sparkles,
  Trash2,
} from "lucide-react";
import {
  xblueviewApi,
  type DashboardDetail,
  type DashboardRecord,
  type DashboardTheme,
  type DrillPathItem,
  type LayoutUpdateItem,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { subscribeDashboardSse } from "@/lib/sse";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { AiDashboardWizard } from "@/components/xblueview/AiDashboardWizard";
import { AskSlideOver } from "@/components/xblueview/AskSlideOver";
import { AssetManagerPanel } from "@/components/xblueview/AssetManagerPanel";
import { BigScreen } from "@/components/xblueview/BigScreen";
import { DashboardGrid } from "@/components/xblueview/DashboardGrid";
import { DashboardSettingsModal } from "@/components/xblueview/DashboardSettingsModal";
import { FilterBar } from "@/components/xblueview/FilterBar";
import {
  buildWidgetFilters as computeScopedWidgetFilters,
  resolveLeafDrillNodeKey,
} from "@/components/xblueview/logic";
import { NodeDrawer } from "@/components/xblueview/NodeDrawer";
import { WidgetEditor } from "@/components/xblueview/WidgetEditor";
import { DEFAULT_CANVAS, type SelectedNodeContext } from "@/components/xblueview/utils";

type GalleryTab = "dashboards" | "assets";

export function XBlueViewPage() {
  const { slug } = useParams<{ slug?: string }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const navigate = useNavigate();

  const isScreenMode = Boolean(slug && searchParams.get("screen") === "1");

  const [isDark, setIsDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  useEffect(() => {
    if (typeof document === "undefined") return;
    const observer = new MutationObserver(() => {
      setIsDark(document.documentElement.classList.contains("dark"));
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  // =========================================================================
  // Gallery Mode (/view)
  // =========================================================================
  const [galleryTab, setGalleryTab] = useState<GalleryTab>("dashboards");
  const [dashboards, setDashboards] = useState<DashboardRecord[]>([]);
  const [galleryLoading, setGalleryLoading] = useState(false);
  const [gallerySearch, setGallerySearch] = useState("");
  const [galleryError, setGalleryError] = useState<string | null>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [newSlug, setNewSlug] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [newTheme, setNewTheme] = useState<DashboardTheme>("light");
  const [creating, setCreating] = useState(false);
  const [aiWizardOpen, setAiWizardOpen] = useState(false);

  const loadDashboards = useCallback(async () => {
    setGalleryLoading(true);
    setGalleryError(null);
    try {
      const res = await xblueviewApi.listDashboards({ limit: 100 });
      setDashboards(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    } finally {
      setGalleryLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!slug) {
      void loadDashboards();
    }
  }, [loadDashboards, slug]);

  const filteredDashboards = useMemo(() => {
    const q = gallerySearch.trim().toLowerCase();
    if (!q) return dashboards;
    return dashboards.filter(
      (d) =>
        d.name.toLowerCase().includes(q) ||
        d.slug.toLowerCase().includes(q) ||
        d.description.toLowerCase().includes(q)
    );
  }, [dashboards, gallerySearch]);

  const handleCreateDashboard = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedName = newName.trim();
    const trimmedSlug = newSlug
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9-]/g, "-");
    if (!trimmedName || !trimmedSlug) {
      setGalleryError("请填写驾驶舱名称与合法 Slug（小写字母/数字/连字符）");
      return;
    }
    setCreating(true);
    setGalleryError(null);
    try {
      const created = await xblueviewApi.createDashboard({
        name: trimmedName,
        slug: trimmedSlug,
        description: newDesc.trim(),
        theme: newTheme,
        canvas: DEFAULT_CANVAS,
      });
      setCreateOpen(false);
      setNewName("");
      setNewSlug("");
      setNewDesc("");
      void navigate(`/view/${created.slug}`);
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleDuplicateDashboard = async (d: DashboardRecord) => {
    try {
      await xblueviewApi.duplicateDashboard(d.id, {
        name: `${d.name} (副本)`,
        slug: `${d.slug}-copy-${Date.now().toString(36).slice(-4)}`,
      });
      await loadDashboards();
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDeleteDashboard = async (d: DashboardRecord) => {
    try {
      await xblueviewApi.deleteDashboard(d.id);
      setDashboards((prev) => prev.filter((item) => item.id !== d.id));
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    }
  };

  // =========================================================================
  // Dashboard Detail / Edit / View / Screen Mode (/view/:slug)
  // =========================================================================
  const [dashboard, setDashboard] = useState<DashboardDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  const [isEditing, setIsEditing] = useState(false);
  const [pendingLayout, setPendingLayout] = useState<LayoutUpdateItem[] | null>(
    null
  );
  const [savingLayout, setSavingLayout] = useState(false);
  const [publishing, setPublishing] = useState(false);

  const [filterValues, setFilterValues] = useState<Record<string, unknown>>({});
  const [drillMap, setDrillMap] = useState<Record<string, DrillPathItem[]>>({});
  const [widgetDataMap, setWidgetDataMap] = useState<
    Record<string, WidgetDataResponse | null>
  >({});
  const [loadingMap, setLoadingMap] = useState<Record<string, boolean>>({});
  const [errorMap, setErrorMap] = useState<Record<string, string | null>>({});

  const [editorOpen, setEditorOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [editingWidget, setEditingWidget] = useState<WidgetRecord | null>(null);
  const [selectedNode, setSelectedNode] = useState<SelectedNodeContext | null>(
    null
  );
  const [askOpen, setAskOpen] = useState(false);
  const [sseConnected, setSseConnected] = useState(false);

  const lastFetchAtRef = useRef<Record<string, number>>({});
  const throttleTimersRef = useRef<Record<string, ReturnType<typeof setTimeout>>>(
    {}
  );
  const dashboardRef = useRef<DashboardDetail | null>(null);
  const filterValuesRef = useRef<Record<string, unknown>>({});
  const drillMapRef = useRef<Record<string, DrillPathItem[]>>({});

  useEffect(() => {
    dashboardRef.current = dashboard;
  }, [dashboard]);

  useEffect(() => {
    filterValuesRef.current = filterValues;
  }, [filterValues]);

  useEffect(() => {
    drillMapRef.current = drillMap;
  }, [drillMap]);

  const buildWidgetFilters = useCallback(
    (
      widgetId: string,
      dash: DashboardDetail,
      currentFilterValues: Record<string, unknown>
    ): Record<string, unknown> =>
      computeScopedWidgetFilters(widgetId, dash.filters, currentFilterValues),
    []
  );

  const fetchWidgetData = useCallback(
    async (
      widgetId: string,
      dashOverride?: DashboardDetail,
      filtersOverride?: Record<string, unknown>,
      drillOverride?: DrillPathItem[]
    ) => {
      const activeDash = dashOverride ?? dashboardRef.current;
      if (!activeDash) return;
      const activeFilters = filtersOverride ?? filterValuesRef.current;
      const activeDrill = drillOverride ?? drillMapRef.current[widgetId] ?? [];
      const scopedFilters = buildWidgetFilters(
        widgetId,
        activeDash,
        activeFilters
      );

      lastFetchAtRef.current[widgetId] = Date.now();
      setLoadingMap((prev) => ({ ...prev, [widgetId]: true }));
      setErrorMap((prev) => ({ ...prev, [widgetId]: null }));
      try {
        const res = await xblueviewApi.executeWidgetData(widgetId, {
          filters: scopedFilters,
          drill_path: activeDrill,
        });
        setWidgetDataMap((prev) => ({ ...prev, [widgetId]: res }));
      } catch (err) {
        setErrorMap((prev) => ({
          ...prev,
          [widgetId]: err instanceof Error ? err.message : String(err),
        }));
      } finally {
        setLoadingMap((prev) => ({ ...prev, [widgetId]: false }));
      }
    },
    [buildWidgetFilters]
  );

  const refreshAllWidgets = useCallback(
    (
      dashOverride?: DashboardDetail,
      filtersOverride?: Record<string, unknown>
    ) => {
      const activeDash = dashOverride ?? dashboardRef.current;
      if (!activeDash) return;
      for (const w of activeDash.widgets) {
        void fetchWidgetData(
          w.id,
          activeDash,
          filtersOverride,
          drillMapRef.current[w.id] ?? []
        );
      }
    },
    [fetchWidgetData]
  );

  const loadDashboardDetail = useCallback(
    async (targetSlug: string) => {
      setDetailLoading(true);
      setDetailError(null);
      try {
        const detail = await xblueviewApi.getDashboardBySlug(targetSlug);
        const initialFilters: Record<string, unknown> = {};
        for (const f of detail.filters ?? []) {
          if (f.default !== undefined && f.default !== null) {
            initialFilters[f.key] = f.default;
          }
        }
        dashboardRef.current = detail;
        filterValuesRef.current = initialFilters;
        drillMapRef.current = {};
        setDashboard(detail);
        setFilterValues(initialFilters);
        setDrillMap({});
        setPendingLayout(null);
        setIsEditing(false);
        for (const w of detail.widgets) {
          void fetchWidgetData(w.id, detail, initialFilters, []);
        }
      } catch (err) {
        setDetailError(err instanceof Error ? err.message : String(err));
        setDashboard(null);
      } finally {
        setDetailLoading(false);
      }
    },
    [fetchWidgetData]
  );

  useEffect(() => {
    if (slug) {
      void loadDashboardDetail(slug);
    } else {
      dashboardRef.current = null;
      setDashboard(null);
    }
  }, [loadDashboardDetail, slug]);

  // Schedule throttled widget refresh (2s per-widget throttle for SSE invalidate)
  const scheduleThrottledWidgetRefresh = useCallback(
    (widgetId: string) => {
      const now = Date.now();
      const last = lastFetchAtRef.current[widgetId] ?? 0;
      const elapsed = now - last;
      if (elapsed >= 2000) {
        const existingTimer = throttleTimersRef.current[widgetId];
        if (existingTimer) {
          clearTimeout(existingTimer);
          delete throttleTimersRef.current[widgetId];
        }
        void fetchWidgetData(widgetId);
      } else if (!throttleTimersRef.current[widgetId]) {
        const delay = 2000 - elapsed;
        throttleTimersRef.current[widgetId] = setTimeout(() => {
          delete throttleTimersRef.current[widgetId];
          void fetchWidgetData(widgetId);
        }, delay);
      }
    },
    [fetchWidgetData]
  );

  // Clean up throttle timers on unmount
  useEffect(() => {
    return () => {
      for (const t of Object.values(throttleTimersRef.current)) {
        clearTimeout(t);
      }
      throttleTimersRef.current = {};
    };
  }, []);

  // SSE Live Invalidation Subscription
  const dashboardId = dashboard?.id ?? null;
  useEffect(() => {
    if (!dashboardId || isEditing) {
      setSseConnected(false);
      return;
    }

    setSseConnected(true);
    const unsubscribe = subscribeDashboardSse({
      dashboardId,
      onInvalidate: (payload) => {
        const activeWidgets = dashboardRef.current?.widgets ?? [];
        const targetIds =
          payload.widget_ids.length > 0
            ? payload.widget_ids
            : activeWidgets
                .filter((w) => w.refresh_mode === "LIVE")
                .map((w) => w.id);

        for (const wid of targetIds) {
          const widget = activeWidgets.find((w) => w.id === wid);
          if (widget && widget.refresh_mode === "LIVE") {
            scheduleThrottledWidgetRefresh(wid);
          }
        }
      },
      onAlert: () => {
        const activeWidgets = dashboardRef.current?.widgets ?? [];
        for (const w of activeWidgets) {
          if (
            w.refresh_mode === "LIVE" &&
            (w.widget_kind === "ALERT_LIST" ||
              w.widget_kind === "SCENE_3D" ||
              w.widget_kind === "FLOOR_PLAN")
          ) {
            scheduleThrottledWidgetRefresh(w.id);
          }
        }
      },
      onReconnected: () => {
        setSseConnected(true);
        refreshAllWidgets();
      },
      onError: () => {
        setSseConnected(false);
      },
    });

    return () => {
      setSseConnected(false);
      unsubscribe();
    };
  }, [dashboardId, isEditing, refreshAllWidgets, scheduleThrottledWidgetRefresh]);

  // INTERVAL polling timer for widgets with refresh_mode === "INTERVAL"
  useEffect(() => {
    if (!dashboard || isEditing) return;
    const intervalWidgets = dashboard.widgets.filter(
      (w) => w.refresh_mode === "INTERVAL"
    );
    if (intervalWidgets.length === 0) return;

    const timer = setInterval(() => {
      const now = Date.now();
      for (const w of intervalWidgets) {
        const last = lastFetchAtRef.current[w.id] ?? 0;
        const intervalMs = Math.max(5, w.refresh_seconds) * 1000;
        if (now - last >= intervalMs) {
          void fetchWidgetData(w.id);
        }
      }
    }, 5000);

    return () => clearInterval(timer);
  }, [dashboard, fetchWidgetData, isEditing]);

  const handleFilterChange = (key: string, value: unknown) => {
    const next = { ...filterValues, [key]: value };
    setFilterValues(next);
    if (!dashboard) return;
    const fDef = dashboard.filters.find((f) => f.key === key);
    if (!fDef) return;
    for (const w of dashboard.widgets) {
      if (fDef.field_map?.[w.id]) {
        void fetchWidgetData(w.id, dashboard, next, drillMap[w.id] ?? []);
      }
    }
  };

  const handleResetFilters = () => {
    if (!dashboard) return;
    const defaults: Record<string, unknown> = {};
    for (const f of dashboard.filters) {
      if (f.default !== undefined && f.default !== null) {
        defaults[f.key] = f.default;
      }
    }
    setFilterValues(defaults);
    refreshAllWidgets(dashboard, defaults);
  };

  const handleCrossFilter = (filterKey: string, value: unknown) => {
    handleFilterChange(filterKey, value);
  };

  const handleDrill = (
    widgetId: string,
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => {
    if (!dashboard) return;
    const widget = dashboard.widgets.find((w) => w.id === widgetId);
    if (!widget || widget.drill.length === 0) return;

    const currentPath = drillMap[widgetId] ?? [];
    const currentLevelDef = widget.drill[currentPath.length];
    if (!currentLevelDef) return;

    // If current drill level configures node_type (leaf entity), open NodeDrawer
    if (currentLevelDef.node_type) {
      const resolvedKey = resolveLeafDrillNodeKey({
        clickedField: field,
        clickedValue: value,
        levelField: currentLevelDef.field || field,
        nodeKeyField: currentLevelDef.node_key_field,
        rowObj,
        widgetData: widgetDataMap[widgetId] ?? null,
      });
      const rawNodeId = rowObj?.node_id ?? rowObj?.id;
      const resolvedNodeId =
        typeof rawNodeId === "string" && rawNodeId.trim().length > 0
          ? rawNodeId.trim()
          : resolvedKey;

      setSelectedNode({
        nodeId: resolvedNodeId,
        typeName: currentLevelDef.node_type,
        nodeKey: resolvedKey,
        title: String(value ?? resolvedKey),
      });
      return;
    }

    // Drill down if there is a next drill level defined
    if (currentPath.length + 1 < widget.drill.length) {
      const nextPath: DrillPathItem[] = [
        ...currentPath,
        { field: currentLevelDef.field || field, value },
      ];
      setDrillMap((prev) => ({ ...prev, [widgetId]: nextPath }));
      void fetchWidgetData(widgetId, dashboard, filterValues, nextPath);
    }
  };

  const handlePopDrill = (widgetId: string, level: number) => {
    if (!dashboard) return;
    const currentPath = drillMap[widgetId] ?? [];
    const nextPath = currentPath.slice(0, Math.max(0, level));
    setDrillMap((prev) => ({ ...prev, [widgetId]: nextPath }));
    void fetchWidgetData(widgetId, dashboard, filterValues, nextPath);
  };

  const handleSaveLayout = async () => {
    if (!dashboard || !pendingLayout) return;
    setSavingLayout(true);
    setDetailError(null);
    try {
      const updatedWidgets = await xblueviewApi.updateLayout(
        dashboard.id,
        pendingLayout
      );
      setDashboard((prev) =>
        prev ? { ...prev, widgets: updatedWidgets } : prev
      );
      setPendingLayout(null);
      setIsEditing(false);
    } catch (err) {
      setDetailError(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingLayout(false);
    }
  };

  const handlePublish = async () => {
    if (!dashboard) return;
    setPublishing(true);
    setDetailError(null);
    try {
      const published = await xblueviewApi.publishDashboard(dashboard.id);
      setDashboard((prev) =>
        prev
          ? {
              ...prev,
              status: published.status,
              version: published.version,
            }
          : prev
      );
    } catch (err) {
      setDetailError(err instanceof Error ? err.message : String(err));
    } finally {
      setPublishing(false);
    }
  };

  const handleDeleteWidget = async (widgetId: string) => {
    try {
      await xblueviewApi.deleteWidget(widgetId);
      setDashboard((prev) =>
        prev
          ? {
              ...prev,
              widgets: prev.widgets.filter((w) => w.id !== widgetId),
            }
          : prev
      );
    } catch (err) {
      setDetailError(err instanceof Error ? err.message : String(err));
    }
  };

  const actionWidgets = useMemo(
    () =>
      (dashboard?.widgets ?? []).filter(
        (w) =>
          w.widget_kind === "ACTION_BUTTON" || w.widget_kind === "ACTION_FORM"
      ),
    [dashboard?.widgets]
  );

  // =========================================================================
  // Render: Gallery View (/view)
  // =========================================================================
  if (!slug) {
    return (
      <div className="space-y-5" data-testid="xblueview-gallery-page">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4">
          <div>
            <h1 className="text-lg font-semibold tracking-tight">
              xBlueView · 孪生驾驶舱
            </h1>
            <p className="text-xs text-muted-foreground mt-0.5">
              20 种可视化组件、AI 一句话建看板、2D/3D 数字孪生场景、动作回写闭环与实时大屏
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="flex items-center rounded-md border border-border bg-muted/40 p-0.5 text-xs">
              <button
                type="button"
                onClick={() => setGalleryTab("dashboards")}
                className={`inline-flex items-center gap-1.5 rounded px-2.5 py-1 font-medium cursor-pointer ${
                  galleryTab === "dashboards"
                    ? "bg-background text-foreground border border-border"
                    : "text-muted-foreground hover:text-foreground"
                }`}
              >
                <FolderKanban className="h-3.5 w-3.5 text-xblue" />
                驾驶舱列表
              </button>
              <button
                type="button"
                onClick={() => setGalleryTab("assets")}
                data-testid="tab-asset-manager"
                className={`inline-flex items-center gap-1.5 rounded px-2.5 py-1 font-medium cursor-pointer ${
                  galleryTab === "assets"
                    ? "bg-background text-foreground border border-border"
                    : "text-muted-foreground hover:text-foreground"
                }`}
              >
                <Layers className="h-3.5 w-3.5 text-xblue" />
                空间资产与 3D 绑定
              </button>
            </div>

            {galleryTab === "dashboards" && (
              <>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setAiWizardOpen(true)}
                  data-testid="open-ai-wizard-btn"
                >
                  <Sparkles className="h-3.5 w-3.5 text-xblue" />
                  AI 一句话建看板
                </Button>
                <Button
                  type="button"
                  onClick={() => setCreateOpen((prev) => !prev)}
                  data-testid="open-create-dashboard-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  新建驾驶舱
                </Button>
              </>
            )}
          </div>
        </div>

        {galleryError && (
          <div className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
            {galleryError}
          </div>
        )}

        {galleryTab === "assets" ? (
          <AssetManagerPanel />
        ) : (
          <>
            {createOpen && (
              <Card className="p-4">
                <form
                  onSubmit={(e) => void handleCreateDashboard(e)}
                  className="grid grid-cols-1 md:grid-cols-12 gap-3 items-end text-xs"
                >
                  <div className="md:col-span-3 space-y-1">
                    <label className="text-muted-foreground block">驾驶舱名称</label>
                    <Input
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      placeholder="例如：供应链总览"
                      className="h-8 text-xs"
                    />
                  </div>
                  <div className="md:col-span-3 space-y-1">
                    <label className="text-muted-foreground block">Slug 标识</label>
                    <Input
                      value={newSlug}
                      onChange={(e) => setNewSlug(e.target.value)}
                      placeholder="supply-overview"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div className="md:col-span-3 space-y-1">
                    <label className="text-muted-foreground block">描述</label>
                    <Input
                      value={newDesc}
                      onChange={(e) => setNewDesc(e.target.value)}
                      placeholder="可选描述"
                      className="h-8 text-xs"
                    />
                  </div>
                  <div className="md:col-span-1 space-y-1">
                    <label className="text-muted-foreground block">主题</label>
                    <select
                      value={newTheme}
                      onChange={(e) =>
                        setNewTheme(e.target.value as DashboardTheme)
                      }
                      className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                    >
                      <option value="light">light</option>
                      <option value="dark">dark</option>
                      <option value="screen">screen</option>
                    </select>
                  </div>
                  <div className="md:col-span-2 flex gap-2">
                    <Button type="submit" disabled={creating} className="flex-1">
                      {creating ? "创建中..." : "创建"}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setCreateOpen(false)}
                    >
                      取消
                    </Button>
                  </div>
                </form>
              </Card>
            )}

            <div className="flex items-center gap-2">
              <div className="relative flex-1 max-w-sm">
                <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-3 top-1/2 -translate-y-1/2" />
                <Input
                  value={gallerySearch}
                  onChange={(e) => setGallerySearch(e.target.value)}
                  placeholder="搜索驾驶舱名称、slug 或描述..."
                  className="h-8 pl-8 text-xs"
                  data-testid="dashboard-search-input"
                />
              </div>
              <Button
                type="button"
                variant="outline"
                onClick={() => void loadDashboards()}
                disabled={galleryLoading}
                className="h-8 px-2.5 text-xs"
              >
                <RefreshCw
                  className={`h-3.5 w-3.5 ${galleryLoading ? "animate-spin" : ""}`}
                />
                刷新
              </Button>
            </div>

            {filteredDashboards.length === 0 ? (
              <Card className="p-10 text-center text-xs text-muted-foreground">
                {galleryLoading
                  ? "正在加载驾驶舱列表..."
                  : "暂无驾驶舱，点击右上角「新建驾驶舱」或「AI 一句话建看板」开始构建。"}
              </Card>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                {filteredDashboards.map((d) => (
                  <Card
                    key={d.id}
                    className="p-4 flex flex-col justify-between gap-3 hover:border-xblue/60 transition-colors"
                    data-testid={`dashboard-card-${d.slug}`}
                  >
                    <div
                      className="space-y-1.5 cursor-pointer"
                      onClick={() => void navigate(`/view/${d.slug}`)}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <h3 className="text-sm font-semibold truncate">{d.name}</h3>
                        <Badge
                          variant={
                            d.status === "PUBLISHED" ? "xblue" : "outline"
                          }
                        >
                          {d.status} v{d.version}
                        </Badge>
                      </div>
                      <div className="font-mono-tech text-[11px] text-muted-foreground">
                        /view/{d.slug} · {d.theme}
                      </div>
                      {d.description && (
                        <p className="text-xs text-muted-foreground line-clamp-2">
                          {d.description}
                        </p>
                      )}
                    </div>

                    <div className="flex items-center justify-between gap-2 pt-2 border-t border-border text-xs">
                      <div className="flex items-center gap-1.5">
                        <Button
                          type="button"
                          variant="outline"
                          className="px-2.5 py-1 text-xs"
                          onClick={() => void navigate(`/view/${d.slug}`)}
                        >
                          打开
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          className="px-2 py-1 text-xs"
                          onClick={() =>
                            void navigate(`/view/${d.slug}?screen=1`)
                          }
                          title="大屏模式"
                        >
                          <Monitor className="h-3.5 w-3.5 text-xblue" />
                        </Button>
                      </div>

                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          onClick={() => void handleDuplicateDashboard(d)}
                          className="rounded p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                          title="复制驾驶舱"
                        >
                          <Copy className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleDeleteDashboard(d)}
                          className="rounded p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                          title="删除驾驶舱"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    </div>
                  </Card>
                ))}
              </div>
            )}
          </>
        )}

        <AiDashboardWizard
          open={aiWizardOpen}
          onClose={() => setAiWizardOpen(false)}
          onGenerated={(created) => {
            setAiWizardOpen(false);
            void navigate(`/view/${created.slug}`);
          }}
        />
      </div>
    );
  }

  // =========================================================================
  // Render: Screen Mode (/view/:slug?screen=1)
  // =========================================================================
  if (isScreenMode && dashboard) {
    return (
      <>
        <BigScreen
          dashboard={dashboard}
          widgetDataMap={widgetDataMap}
          loadingMap={loadingMap}
          errorMap={errorMap}
          drillMap={drillMap}
          sseConnected={sseConnected}
          selectedNodeId={selectedNode?.nodeId ?? null}
          filterValues={filterValues}
          onExitScreen={() => setSearchParams({})}
          onRefreshWidget={(wid) => void fetchWidgetData(wid)}
          onDrill={handleDrill}
          onPopDrill={handlePopDrill}
          onCrossFilter={handleCrossFilter}
          onSelectNode={setSelectedNode}
          onActionSuccess={() => refreshAllWidgets()}
        />
        <NodeDrawer
          selected={selectedNode}
          actionWidgets={actionWidgets}
          onClose={() => setSelectedNode(null)}
          onActionExecuted={() => refreshAllWidgets()}
        />
      </>
    );
  }

  const forceDarkCanvas =
    dashboard?.theme === "dark" || dashboard?.theme === "screen";
  const effectiveDark = Boolean(isDark || forceDarkCanvas);

  // =========================================================================
  // Render: Dashboard Detail View / Edit (/view/:slug)
  // =========================================================================
  return (
    <div className="space-y-4" data-testid="xblueview-detail-page">
      {/* Top Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        <div className="flex items-center gap-3 min-w-0">
          <Button
            type="button"
            variant="ghost"
            className="px-2 py-1"
            onClick={() => void navigate("/view")}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h1 className="text-lg font-semibold tracking-tight truncate">
                {dashboard?.name ?? `驾驶舱 · ${slug}`}
              </h1>
              {dashboard && (
                <Badge
                  variant={
                    dashboard.status === "PUBLISHED" ? "xblue" : "outline"
                  }
                >
                  {dashboard.status} v{dashboard.version}
                </Badge>
              )}
              {!isEditing && dashboard && (
                <span className="inline-flex items-center gap-1 rounded border border-border px-2 py-0.5 text-[10px] font-mono-tech text-muted-foreground">
                  <Radio
                    className={`h-2.5 w-2.5 ${
                      sseConnected ? "text-xblue" : "text-muted-foreground"
                    }`}
                  />
                  {sseConnected ? "SSE LIVE" : "POLL"}
                </span>
              )}
            </div>
            {dashboard?.description && (
              <p className="text-xs text-muted-foreground truncate">
                {dashboard.description}
              </p>
            )}
          </div>
        </div>

        {dashboard && (
          <div className="flex flex-wrap items-center gap-2">
            {!isEditing ? (
              <>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => refreshAllWidgets()}
                  data-testid="refresh-all-widgets-btn"
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  刷新
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setAskOpen(true)}
                  data-testid="open-ask-slideover-btn"
                >
                  <MessageSquare className="h-3.5 w-3.5 text-xblue" />
                  问一问
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setSearchParams({ screen: "1" })}
                  data-testid="enter-big-screen-btn"
                >
                  <Monitor className="h-3.5 w-3.5 text-xblue" />
                  大屏模式
                </Button>
                <Button
                  type="button"
                  onClick={() => setIsEditing(true)}
                  data-testid="toggle-edit-mode-btn"
                >
                  <Edit3 className="h-3.5 w-3.5" />
                  编辑布局
                </Button>
              </>
            ) : (
              <>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setSettingsOpen(true)}
                  data-testid="open-dashboard-settings-btn"
                >
                  <Settings2 className="h-3.5 w-3.5 text-xblue" />
                  驾驶舱设置
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => {
                    setEditingWidget(null);
                    setEditorOpen(true);
                  }}
                  data-testid="add-widget-btn"
                >
                  <Plus className="h-3.5 w-3.5 text-xblue" />
                  添加组件
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  disabled={!pendingLayout || savingLayout}
                  onClick={() => void handleSaveLayout()}
                  data-testid="save-layout-btn"
                >
                  <Save className="h-3.5 w-3.5" />
                  {savingLayout ? "保存中..." : "保存布局"}
                </Button>
                <Button
                  type="button"
                  disabled={publishing}
                  onClick={() => void handlePublish()}
                  data-testid="publish-dashboard-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {publishing ? "发布中..." : "发布驾驶舱"}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setIsEditing(false);
                    setPendingLayout(null);
                  }}
                >
                  完成
                </Button>
              </>
            )}
          </div>
        )}
      </div>

      {detailError && (
        <div
          className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
          data-testid="view-error-banner"
        >
          {detailError}
        </div>
      )}

      {detailLoading && !dashboard ? (
        <Card className="p-12 text-center text-xs text-muted-foreground">
          正在加载孪生驾驶舱...
        </Card>
      ) : dashboard ? (
        <>
          <div
            data-testid="dashboard-canvas-scope"
            className={
              forceDarkCanvas
                ? "dark bg-zinc-950 text-zinc-100 rounded-lg border border-zinc-800 p-3 space-y-4"
                : "space-y-4"
            }
          >
            {!isEditing && (
              <FilterBar
                filters={dashboard.filters}
                values={filterValues}
                onChange={handleFilterChange}
                onReset={handleResetFilters}
              />
            )}

            <DashboardGrid
              widgets={dashboard.widgets}
              canvas={dashboard.canvas}
              widgetDataMap={widgetDataMap}
              loadingMap={loadingMap}
              errorMap={errorMap}
              drillMap={drillMap}
              isEditing={isEditing}
              isDark={effectiveDark}
              selectedNodeId={selectedNode?.nodeId ?? null}
              filterValues={filterValues}
              onLayoutChange={(items) => setPendingLayout(items)}
              onRefreshWidget={(wid) => void fetchWidgetData(wid)}
              onEditWidget={(w) => {
                setEditingWidget(w);
                setEditorOpen(true);
              }}
              onDeleteWidget={(wid) => void handleDeleteWidget(wid)}
              onDrill={handleDrill}
              onPopDrill={handlePopDrill}
              onCrossFilter={handleCrossFilter}
              onSelectNode={setSelectedNode}
              onActionSuccess={() => refreshAllWidgets()}
            />
          </div>

          {settingsOpen && (
            <DashboardSettingsModal
              dashboard={dashboard}
              onClose={() => setSettingsOpen(false)}
              onSaved={(updatedRecord) => {
                setSettingsOpen(false);
                const nextDetail: DashboardDetail = {
                  ...dashboard,
                  name: updatedRecord.name,
                  description: updatedRecord.description,
                  theme: updatedRecord.theme,
                  canvas: updatedRecord.canvas,
                  filters: updatedRecord.filters,
                  status: updatedRecord.status,
                  version: updatedRecord.version,
                  updated_at: updatedRecord.updated_at,
                };
                dashboardRef.current = nextDetail;
                setDashboard(nextDetail);
                refreshAllWidgets(nextDetail);
              }}
            />
          )}

          {editorOpen && (
            <WidgetEditor
              dashboardId={dashboard.id}
              editingWidget={editingWidget}
              existingWidgets={dashboard.widgets}
              isDark={effectiveDark}
              onClose={() => {
                setEditorOpen(false);
                setEditingWidget(null);
              }}
              onSaved={(saved, isNewWidget) => {
                setEditorOpen(false);
                setEditingWidget(null);
                setDashboard((prev) => {
                  if (!prev) return prev;
                  const nextWidgets = isNewWidget
                    ? [...prev.widgets, saved]
                    : prev.widgets.map((w) => (w.id === saved.id ? saved : w));
                  return { ...prev, widgets: nextWidgets };
                });
                void fetchWidgetData(saved.id, dashboard);
              }}
            />
          )}

          <NodeDrawer
            selected={selectedNode}
            actionWidgets={actionWidgets}
            onClose={() => setSelectedNode(null)}
            onActionExecuted={() => refreshAllWidgets()}
          />

          <AskSlideOver
            open={askOpen}
            dashboard={dashboard}
            filterValues={filterValues}
            drillMap={drillMap}
            isDark={effectiveDark}
            onClose={() => setAskOpen(false)}
            onAddedWidget={(created) => {
              setDashboard((prev) =>
                prev
                  ? { ...prev, widgets: [...prev.widgets, created] }
                  : prev
              );
              void fetchWidgetData(created.id, dashboard);
            }}
          />
        </>
      ) : null}
    </div>
  );
}
