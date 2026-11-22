import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import {
  ArrowLeft,
  CheckCircle2,
  Copy,
  Download,
  Edit3,
  FileArchive,
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
  Share2,
  ShieldCheck,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type CockpitZoneLayout,
  type DashboardDetail,
  type DashboardPreflightReport,
  type DashboardRecord,
  type DashboardTheme,
  type DrillPathItem,
  type LayoutUpdateItem,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { subscribeDashboardSse } from "@/lib/sse";
import { useTheme } from "@/lib/theme";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  IconButton,
  Input,
  PageHeader,
  SegmentedControl,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { AiDashboardWizard } from "@/components/xblueview/AiDashboardWizard";
import { AskSlideOver } from "@/components/xblueview/AskSlideOver";
import { AssetManagerPanel } from "@/components/xblueview/AssetManagerPanel";
import { BigScreen } from "@/components/xblueview/BigScreen";
import { DashboardGrid } from "@/components/xblueview/DashboardGrid";
import { DashboardSettingsModal } from "@/components/xblueview/DashboardSettingsModal";
import {
  ExportDialog,
  type ExportDialogTarget,
} from "@/components/xblueview/ExportDialog";
import { ExportList } from "@/components/xblueview/ExportList";
import { FilterBar } from "@/components/xblueview/FilterBar";
import {
  buildWidgetFilters as computeScopedWidgetFilters,
  resolveLeafDrillNodeKey,
  sanitizeExportFilters,
} from "@/components/xblueview/logic";
import { NodeDrawer } from "@/components/xblueview/NodeDrawer";
import { PublishPreflightModal } from "@/components/xblueview/PublishPreflightModal";
import {
  ShareDialog,
  type ShareDialogTarget,
} from "@/components/xblueview/ShareDialog";
import { WidgetEditor } from "@/components/xblueview/WidgetEditor";
import {
  DEFAULT_CANVAS,
  getWidgetKindLabel,
  type SelectedNodeContext,
} from "@/components/xblueview/utils";

type GalleryTab = "dashboards" | "assets";

export function XBlueViewPage() {
  const { slug } = useParams<{ slug?: string }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const navigate = useNavigate();

  const { tx, isZh } = useI18n();
  const { isDark } = useTheme();
  const { can } = usePrincipal();
  const canEdit = can("VIEW_EDIT");
  const canShare = can("VIEW_SHARE");
  const canExport = can("EXPORT_DATA");

  const isScreenMode = Boolean(slug && searchParams.get("screen") === "1");

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
  const [newZoneLayout, setNewZoneLayout] =
    useState<CockpitZoneLayout>("FREE_GRID");
  const [creating, setCreating] = useState(false);
  const [aiWizardOpen, setAiWizardOpen] = useState(false);
  const [pendingDeleteDashboard, setPendingDeleteDashboard] =
    useState<DashboardRecord | null>(null);
  const [deletingDashboard, setDeletingDashboard] = useState(false);

  const wizardParam = searchParams.get("wizard")?.trim() ?? "";
  const wizardSubgraph = useMemo(() => {
    const raw = searchParams.get("subgraph")?.trim() ?? "";
    return raw.length > 0 ? raw : null;
  }, [searchParams]);
  const wizardNodeTypesRaw = searchParams.get("node_types")?.trim() ?? "";
  const wizardSeedNodeTypes = useMemo(
    () =>
      wizardNodeTypesRaw
        ? wizardNodeTypesRaw
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean)
        : [],
    [wizardNodeTypesRaw]
  );

  useEffect(() => {
    if (!slug && wizardParam === "ai") {
      setGalleryTab("dashboards");
      setAiWizardOpen(true);
    }
  }, [slug, wizardParam]);

  const clearWizardSearchParams = useCallback(() => {
    if (
      searchParams.has("wizard") ||
      searchParams.has("subgraph") ||
      searchParams.has("node_types")
    ) {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.delete("wizard");
          next.delete("subgraph");
          next.delete("node_types");
          return next;
        },
        { replace: true }
      );
    }
  }, [searchParams, setSearchParams]);

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
    if (!canEdit) return;
    const trimmedName = newName.trim();
    const trimmedSlug = newSlug
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9-]/g, "-");
    if (!trimmedName || !trimmedSlug) {
      setGalleryError(
        tx(
          "驾驶舱名称与合法 Slug（小写字母/数字/连字符）不能为空",
          "Dashboard name and valid slug (lowercase letters, digits, hyphens) are required"
        )
      );
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
        zone_layout: newZoneLayout,
        ...(newZoneLayout === "FREE_GRID" ? { canvas: DEFAULT_CANVAS } : {}),
      });
      setCreateOpen(false);
      setNewName("");
      setNewSlug("");
      setNewDesc("");
      setNewZoneLayout("FREE_GRID");
      void navigate(`/view/${created.slug}`);
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleDuplicateDashboard = async (d: DashboardRecord) => {
    if (!canEdit) return;
    try {
      await xblueviewApi.duplicateDashboard(d.id, {
        name: tx(`${d.name} (副本)`, `${d.name} (Copy)`),
        slug: `${d.slug}-copy-${Date.now().toString(36).slice(-4)}`,
      });
      await loadDashboards();
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    }
  };

  const confirmDeleteDashboard = async () => {
    if (!pendingDeleteDashboard || !canEdit) return;
    setDeletingDashboard(true);
    try {
      await xblueviewApi.deleteDashboard(pendingDeleteDashboard.id);
      setDashboards((prev) =>
        prev.filter((item) => item.id !== pendingDeleteDashboard.id)
      );
      setPendingDeleteDashboard(null);
    } catch (err) {
      setGalleryError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingDashboard(false);
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
  const [pendingDeleteWidgetId, setPendingDeleteWidgetId] = useState<
    string | null
  >(null);
  const [deletingWidget, setDeletingWidget] = useState(false);
  const [selectedNode, setSelectedNode] = useState<SelectedNodeContext | null>(
    null
  );
  const [askOpen, setAskOpen] = useState(false);
  const [sseConnected, setSseConnected] = useState(false);
  const [exportTarget, setExportTarget] = useState<ExportDialogTarget | null>(
    null
  );
  const [exportListOpen, setExportListOpen] = useState(false);
  const [highlightExportJobId, setHighlightExportJobId] = useState<
    string | null
  >(null);
  const [shareTarget, setShareTarget] = useState<ShareDialogTarget | null>(
    null
  );
  const [preflightOpen, setPreflightOpen] = useState(false);
  const [preflightLoading, setPreflightLoading] = useState(false);
  const [preflightError, setPreflightError] = useState<string | null>(null);
  const [preflightReport, setPreflightReport] =
    useState<DashboardPreflightReport | null>(null);
  const [preflightIntent, setPreflightIntent] = useState<
    "CHECK" | "PUBLISH" | "SHARE"
  >("CHECK");

  const lastFetchAtRef = useRef<Record<string, number>>({});
  const throttleTimersRef = useRef<
    Record<string, ReturnType<typeof setTimeout>>
  >({});
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
  }, [
    dashboardId,
    isEditing,
    refreshAllWidgets,
    scheduleThrottledWidgetRefresh,
  ]);

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
    if (!dashboard || !pendingLayout || !canEdit) return;
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

  const openPreflightModal = async (intent: "CHECK" | "PUBLISH" | "SHARE") => {
    if (!dashboard) return;
    setPreflightIntent(intent);
    setPreflightOpen(true);
    setPreflightLoading(true);
    setPreflightError(null);
    setPreflightReport(null);
    try {
      const rep = await xblueviewApi.preflightDashboard(dashboard.id);
      setPreflightReport(rep);
    } catch (err) {
      setPreflightError(err instanceof Error ? err.message : String(err));
    } finally {
      setPreflightLoading(false);
    }
  };

  const handleConfirmPublishFromPreflight = async () => {
    if (!dashboard || !preflightReport || preflightReport.blocking_count > 0) {
      return;
    }
    setPublishing(true);
    setPreflightError(null);
    setDetailError(null);
    try {
      let nextStatus = dashboard.status;
      if (canEdit && (preflightIntent !== "SHARE" || dashboard.status !== "PUBLISHED")) {
        const published = await xblueviewApi.publishDashboard(dashboard.id);
        nextStatus = published.status;
        setDashboard((prev) =>
          prev
            ? {
                ...prev,
                status: published.status,
                version: published.version,
              }
            : prev
        );
      }
      setPreflightOpen(false);
      if (preflightIntent === "SHARE" && canShare) {
        setShareTarget({
          targetKind: "DASHBOARD",
          targetId: dashboard.id,
          targetName: dashboard.name,
          dashboardStatus: nextStatus,
          dashboardFilters: dashboard.filters,
          activeFilterValues: filterValues,
        });
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setPreflightError(msg);
      setDetailError(msg);
    } finally {
      setPublishing(false);
    }
  };

  const handleLocateWidgetFromPreflight = (widgetId: string) => {
    setPreflightOpen(false);
    if (!dashboard) return;
    if (canEdit) {
      setIsEditing(true);
    }
    const targetWidget =
      dashboard.widgets.find((w) => w.id === widgetId) ?? null;
    if (typeof document !== "undefined") {
      requestAnimationFrame(() => {
        const el = document.querySelector(
          `[data-testid="widget-frame-${widgetId}"]`
        );
        el?.scrollIntoView({ behavior: "smooth", block: "center" });
      });
    }
    if (targetWidget && canEdit) {
      setEditingWidget(targetWidget);
      setEditorOpen(true);
    }
  };

  const handlePublish = async () => {
    await openPreflightModal("PUBLISH");
  };

  const confirmDeleteWidget = async () => {
    if (!pendingDeleteWidgetId || !canEdit) return;
    setDeletingWidget(true);
    try {
      await xblueviewApi.deleteWidget(pendingDeleteWidgetId);
      setDashboard((prev) =>
        prev
          ? {
              ...prev,
              widgets: prev.widgets.filter(
                (w) => w.id !== pendingDeleteWidgetId
              ),
            }
          : prev
      );
      setPendingDeleteWidgetId(null);
    } catch (err) {
      setDetailError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingWidget(false);
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
        <PageHeader
          title={tx("xBlueView · 孪生驾驶舱", "xBlueView · Twin Cockpit")}
          actions={
            <>
              <SegmentedControl<GalleryTab>
                value={galleryTab}
                onChange={setGalleryTab}
                items={[
                  {
                    key: "dashboards",
                    label: tx("驾驶舱列表", "Dashboards"),
                    icon: <FolderKanban className="h-3.5 w-3.5 text-xblue" />,
                  },
                  {
                    key: "assets",
                    label: tx("空间资产", "Spatial Assets"),
                    icon: <Layers className="h-3.5 w-3.5 text-xblue" />,
                    testId: "tab-asset-manager",
                  },
                ]}
              />

              {galleryTab === "dashboards" && canEdit && (
                <>
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => setAiWizardOpen(true)}
                    data-testid="open-ai-wizard-btn"
                  >
                    <Sparkles className="h-3.5 w-3.5 text-xblue" />
                    {tx("AI 生成看板", "AI Builder")}
                  </Button>
                  <Button
                    type="button"
                    onClick={() => setCreateOpen((prev) => !prev)}
                    data-testid="open-create-dashboard-btn"
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {tx("新建驾驶舱", "New Dashboard")}
                  </Button>
                </>
              )}
            </>
          }
        />

        {galleryError && (
          <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
            {galleryError}
          </div>
        )}

        {galleryTab === "assets" ? (
          <AssetManagerPanel />
        ) : (
          <>
            {createOpen && canEdit && (
              <Card className="p-4">
                <form
                  onSubmit={(e) => void handleCreateDashboard(e)}
                  className="grid grid-cols-1 md:grid-cols-12 gap-3 items-end text-xs"
                >
                  <div className="md:col-span-2 space-y-1">
                    <label className="text-muted-foreground block">
                      {tx("驾驶舱名称", "Name")}
                    </label>
                    <Input
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      placeholder={tx("供应链总览", "Supply Overview")}
                      className="h-8 text-xs"
                    />
                  </div>
                  <div className="md:col-span-2 space-y-1">
                    <label className="text-muted-foreground block">Slug</label>
                    <Input
                      value={newSlug}
                      onChange={(e) => setNewSlug(e.target.value)}
                      placeholder="supply-overview"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                  <div className="md:col-span-2 space-y-1">
                    <label className="text-muted-foreground block">
                      {tx("描述", "Description")}
                    </label>
                    <Input
                      value={newDesc}
                      onChange={(e) => setNewDesc(e.target.value)}
                      placeholder={tx(
                        "华东供应链实时监控",
                        "East China supply monitoring"
                      )}
                      className="h-8 text-xs"
                    />
                  </div>
                  <div className="md:col-span-3 space-y-1">
                    <label className="text-muted-foreground block">
                      {tx("布局模式", "Layout")}
                    </label>
                    <SegmentedControl<CockpitZoneLayout>
                      value={newZoneLayout}
                      onChange={setNewZoneLayout}
                      className="w-full"
                      items={[
                        {
                          key: "FREE_GRID",
                          label: tx("自由画布", "Free Grid"),
                          testId: "zone-layout-free-grid-btn",
                        },
                        {
                          key: "STANDARD_FIVE_ZONE",
                          label: tx("五区驾驶舱", "Five-Zone"),
                          testId: "zone-layout-five-zone-btn",
                        },
                      ]}
                    />
                  </div>
                  <div className="md:col-span-1 space-y-1">
                    <label className="text-muted-foreground block">
                      {tx("主题", "Theme")}
                    </label>
                    <Select
                      value={newTheme}
                      onChange={(e) =>
                        setNewTheme(e.target.value as DashboardTheme)
                      }
                      className="h-8 w-full text-xs"
                    >
                      <option value="light">{tx("浅色", "Light")}</option>
                      <option value="dark">{tx("深色", "Dark")}</option>
                      <option value="screen">{tx("大屏", "Screen")}</option>
                    </Select>
                  </div>
                  <div className="md:col-span-2 flex gap-2">
                    <Button type="submit" disabled={creating} className="flex-1">
                      {creating
                        ? tx("创建中…", "Creating…")
                        : tx("创建", "Create")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setCreateOpen(false)}
                    >
                      {tx("取消", "Cancel")}
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
                  placeholder={tx(
                    "名称、Slug 或描述…",
                    "Name, slug, or description…"
                  )}
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
                  className={`h-3.5 w-3.5 ${
                    galleryLoading ? "animate-spin" : ""
                  }`}
                />
                {tx("刷新", "Refresh")}
              </Button>
            </div>

            {filteredDashboards.length === 0 ? (
              <EmptyState
                title={
                  galleryLoading
                    ? tx("加载中…", "Loading…")
                    : tx("暂无驾驶舱", "No dashboards")
                }
              />
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
                        <h3 className="text-sm font-semibold truncate">
                          {d.name}
                        </h3>
                        <div className="flex items-center gap-1 shrink-0">
                          <StatusBadge status={d.status} />
                          <Badge variant="outline">v{d.version}</Badge>
                        </div>
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
                          size="sm"
                          onClick={() => void navigate(`/view/${d.slug}`)}
                        >
                          {tx("打开", "Open")}
                        </Button>
                        <IconButton
                          size="sm"
                          onClick={() =>
                            void navigate(`/view/${d.slug}?screen=1`)
                          }
                          aria-label={tx("大屏模式", "Big Screen")}
                          title={tx("大屏模式", "Big Screen")}
                        >
                          <Monitor className="h-3.5 w-3.5 text-xblue" />
                        </IconButton>
                      </div>

                      {canEdit && (
                        <div className="flex items-center gap-1">
                          <IconButton
                            size="sm"
                            onClick={() => void handleDuplicateDashboard(d)}
                            aria-label={tx("复制驾驶舱", "Duplicate")}
                            title={tx("复制驾驶舱", "Duplicate")}
                          >
                            <Copy className="h-3.5 w-3.5" />
                          </IconButton>
                          <IconButton
                            size="sm"
                            onClick={() => setPendingDeleteDashboard(d)}
                            aria-label={tx("删除驾驶舱", "Delete")}
                            title={tx("删除驾驶舱", "Delete")}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </IconButton>
                        </div>
                      )}
                    </div>
                  </Card>
                ))}
              </div>
            )}
          </>
        )}

        <AiDashboardWizard
          open={aiWizardOpen}
          initialSubgraphSlug={wizardSubgraph}
          initialSeedNodeTypes={wizardSeedNodeTypes}
          onClose={() => {
            setAiWizardOpen(false);
            clearWizardSearchParams();
          }}
          onGenerated={(created) => {
            setAiWizardOpen(false);
            clearWizardSearchParams();
            void navigate(`/view/${created.slug}`);
          }}
        />

        <ConfirmDialog
          open={Boolean(pendingDeleteDashboard)}
          onClose={() => setPendingDeleteDashboard(null)}
          onConfirm={() => void confirmDeleteDashboard()}
          loading={deletingDashboard}
          title={tx("删除驾驶舱", "Delete Dashboard")}
          description={
            pendingDeleteDashboard
              ? tx(
                  `删除「${pendingDeleteDashboard.name}」后，其包含的全部组件与分享链接将同步移除。`,
                  `Deleting "${pendingDeleteDashboard.name}" removes all of its widgets and share links.`
                )
              : ""
          }
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
  const pendingDeleteWidgetObj = pendingDeleteWidgetId
    ? dashboard?.widgets.find((w) => w.id === pendingDeleteWidgetId) ?? null
    : null;

  // =========================================================================
  // Render: Dashboard Detail View / Edit (/view/:slug)
  // =========================================================================
  return (
    <div className="space-y-4" data-testid="xblueview-detail-page">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        <div className="flex items-center gap-2.5 min-w-0">
          <IconButton
            size="sm"
            onClick={() => void navigate("/view")}
            aria-label={tx("返回列表", "Back")}
            title={tx("返回列表", "Back")}
          >
            <ArrowLeft className="h-4 w-4" />
          </IconButton>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="text-base font-semibold tracking-tight truncate">
                {dashboard?.name ?? `${tx("驾驶舱", "Dashboard")} · ${slug}`}
              </h1>
              {dashboard && (
                <>
                  <StatusBadge status={dashboard.status} />
                  <Badge variant="outline">v{dashboard.version}</Badge>
                </>
              )}
              {!isEditing && dashboard && (
                <Badge variant="outline" className="gap-1">
                  <Radio
                    className={`h-2.5 w-2.5 ${
                      sseConnected ? "text-xblue" : "text-muted-foreground"
                    }`}
                  />
                  {sseConnected
                    ? tx("实时推送", "LIVE")
                    : tx("轮询", "POLL")}
                </Badge>
              )}
            </div>
            {dashboard?.description && (
              <p className="text-xs text-muted-foreground truncate mt-0.5">
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
                  {tx("刷新", "Refresh")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setAskOpen(true)}
                  data-testid="open-ask-slideover-btn"
                >
                  <MessageSquare className="h-3.5 w-3.5 text-xblue" />
                  {tx("智能问数", "Ask")}
                </Button>
                {canExport && (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() =>
                        setExportTarget({
                          kind: "DASHBOARD",
                          dashboardId: dashboard.id,
                          dashboardName: dashboard.name,
                          filters: sanitizeExportFilters(filterValues),
                        })
                      }
                      data-testid="open-export-dialog-btn"
                    >
                      <Download className="h-3.5 w-3.5 text-xblue" />
                      {tx("出品导出", "Export")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setExportListOpen(true)}
                      data-testid="open-export-list-btn"
                    >
                      <FileArchive className="h-3.5 w-3.5 text-xblue" />
                      {tx("出品中心", "Exports")}
                    </Button>
                  </>
                )}
                {(canEdit || canShare) && (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => void openPreflightModal("CHECK")}
                    data-testid="view-preflight-btn"
                  >
                    <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
                    {tx("自检", "Preflight")}
                  </Button>
                )}
                {canShare && (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => void openPreflightModal("SHARE")}
                    data-testid="open-share-dialog-btn"
                  >
                    <Share2 className="h-3.5 w-3.5 text-xblue" />
                    {tx("安全分享", "Share")}
                  </Button>
                )}
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setSearchParams({ screen: "1" })}
                  data-testid="enter-big-screen-btn"
                >
                  <Monitor className="h-3.5 w-3.5 text-xblue" />
                  {tx("大屏模式", "Big Screen")}
                </Button>
                {canEdit && (
                  <Button
                    type="button"
                    onClick={() => setIsEditing(true)}
                    data-testid="toggle-edit-mode-btn"
                  >
                    <Edit3 className="h-3.5 w-3.5" />
                    {tx("编辑布局", "Edit Layout")}
                  </Button>
                )}
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
                  {tx("驾驶舱设置", "Settings")}
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
                  {tx("添加组件", "Add Widget")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  disabled={!pendingLayout || savingLayout}
                  onClick={() => void handleSaveLayout()}
                  data-testid="save-layout-btn"
                >
                  <Save className="h-3.5 w-3.5" />
                  {savingLayout
                    ? tx("保存中…", "Saving…")
                    : tx("保存布局", "Save Layout")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => void openPreflightModal("CHECK")}
                  data-testid="view-preflight-btn"
                >
                  <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
                  {tx("自检", "Preflight")}
                </Button>
                <Button
                  type="button"
                  disabled={publishing}
                  onClick={() => void handlePublish()}
                  data-testid="publish-dashboard-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {publishing
                    ? tx("发布中…", "Publishing…")
                    : tx("发布驾驶舱", "Publish")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    setIsEditing(false);
                    setPendingLayout(null);
                  }}
                >
                  {tx("完成", "Done")}
                </Button>
              </>
            )}
          </div>
        )}
      </div>

      {detailError && (
        <div
          className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300"
          data-testid="view-error-banner"
        >
          {detailError}
        </div>
      )}

      {detailLoading && !dashboard ? (
        <Card className="p-12 text-center text-xs text-muted-foreground">
          {tx("加载中…", "Loading…")}
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
              onEditWidget={
                canEdit
                  ? (w) => {
                      setEditingWidget(w);
                      setEditorOpen(true);
                    }
                  : undefined
              }
              onDeleteWidget={
                canEdit ? (wid) => setPendingDeleteWidgetId(wid) : undefined
              }
              onExportWidget={
                canExport
                  ? (w) =>
                      setExportTarget({
                        kind: "WIDGET",
                        dashboardId: dashboard.id,
                        dashboardName: dashboard.name,
                        widgetId: w.id,
                        widgetTitle:
                          w.title || getWidgetKindLabel(w.widget_kind, isZh),
                        filters: buildWidgetFilters(
                          w.id,
                          dashboard,
                          filterValues
                        ),
                        drillPath: drillMap[w.id] ?? [],
                      })
                  : undefined
              }
              onDrill={handleDrill}
              onPopDrill={handlePopDrill}
              onCrossFilter={handleCrossFilter}
              onSelectNode={setSelectedNode}
              onActionSuccess={() => refreshAllWidgets()}
            />
          </div>

          {settingsOpen && (
            <DashboardSettingsModal
              open={settingsOpen}
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
              open={editorOpen}
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

          <ConfirmDialog
            open={Boolean(pendingDeleteWidgetId)}
            onClose={() => setPendingDeleteWidgetId(null)}
            onConfirm={() => void confirmDeleteWidget()}
            loading={deletingWidget}
            title={tx("移除组件", "Delete Widget")}
            description={
              pendingDeleteWidgetObj
                ? tx(
                    `将从当前驾驶舱移除组件「${pendingDeleteWidgetObj.title}」。`,
                    `Remove widget "${pendingDeleteWidgetObj.title}" from this dashboard.`
                  )
                : tx(
                    "将从当前驾驶舱移除该组件。",
                    "Remove this widget from the dashboard."
                  )
            }
          />

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

          <ExportDialog
            open={Boolean(exportTarget)}
            target={exportTarget}
            onClose={() => setExportTarget(null)}
            onExportCreated={(job) => {
              setExportTarget(null);
              setHighlightExportJobId(job.id);
              setExportListOpen(true);
            }}
          />

          <ExportList
            open={exportListOpen}
            highlightJobId={highlightExportJobId}
            onClose={() => {
              setExportListOpen(false);
              setHighlightExportJobId(null);
            }}
            onShareExportFile={(job) => {
              setExportListOpen(false);
              setShareTarget({
                targetKind: "EXPORT_FILE",
                targetId: job.id,
                targetName:
                  job.file_name ||
                  job.options?.title ||
                  `${tx("导出文件", "Export")} · ${job.export_format}`,
              });
            }}
          />

          <ShareDialog
            open={Boolean(shareTarget)}
            target={
              shareTarget?.targetKind === "DASHBOARD"
                ? {
                    ...shareTarget,
                    dashboardStatus: dashboard.status,
                    dashboardFilters: dashboard.filters,
                    activeFilterValues: filterValues,
                  }
                : shareTarget
            }
            onClose={() => setShareTarget(null)}
            onPublishDashboard={() => {
              setShareTarget(null);
              return openPreflightModal("SHARE");
            }}
          />

          <PublishPreflightModal
            open={preflightOpen}
            dashboardName={dashboard.name}
            loading={preflightLoading}
            confirming={publishing}
            error={preflightError}
            report={preflightReport}
            onClose={() => setPreflightOpen(false)}
            onLocateWidget={handleLocateWidgetFromPreflight}
            onConfirmPublish={() => void handleConfirmPublishFromPreflight()}
          />
        </>
      ) : null}
    </div>
  );
}
