import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router";
import {
  AlertTriangle,
  Clock,
  Download,
  FileText,
  GitBranch,
  KeyRound,
  Lock,
  Moon,
  ShieldCheck,
  Sun,
} from "lucide-react";
import { ApiError } from "@/lib/api/client";
import type { ExportFormat } from "@/lib/api/enums";
import {
  setShareAssetScope,
  xblueviewShareApi,
  type DashboardDetail,
  type DrillPathItem,
  type SharePublicResponse,
  type WidgetDataResponse,
} from "@/lib/api/xblueview";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { DashboardGrid } from "@/components/xblueview/DashboardGrid";
import { FilterBar } from "@/components/xblueview/FilterBar";
import {
  buildWidgetFilters,
  mergeLockedShareFilters,
} from "@/components/xblueview/logic";
import { formatDisplayValue, isRecord } from "@/components/xblueview/utils";

const SHARE_EXPORT_FORMATS: readonly ExportFormat[] = [
  "PDF",
  "XLSX",
  "CSV",
  "DOCX",
  "PPTX",
  "MARKDOWN",
];

interface ShareErrorView {
  status: number | null;
  title: string;
  detail: string;
}

function classifySharePageError(err: unknown): ShareErrorView {
  if (err instanceof ApiError) {
    if (err.status === 404) {
      return {
        status: 404,
        title: "分享链接无效、已过期或已撤销 (404)",
        detail:
          "该安全分享链接不存在、已超过有效期、已被创建者撤销，或已达到最大访问次数上限。",
      };
    }
    if (err.status === 429) {
      return {
        status: 429,
        title: "访问请求过于频繁 (429)",
        detail: "当前 IP 访问或口令验证频率已达安全上限，请稍后再试。",
      };
    }
    if (err.status === 401) {
      return {
        status: 401,
        title: "访问会话已失效 (401)",
        detail: "口令验证会话已过期，请重新输入访问口令。",
      };
    }
    return {
      status: err.status,
      title: `加载分享内容失败 (${err.errorCode})`,
      detail: err.message,
    };
  }
  return {
    status: null,
    title: "无法连接分享服务",
    detail: err instanceof Error ? err.message : String(err),
  };
}

function classifyUnlockError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 401) {
      return "访问口令不正确，请核对后重新输入 (401)";
    }
    if (err.status === 429) {
      return "口令尝试次数过多，请稍后再试 (429)";
    }
    if (err.status === 404) {
      return "分享链接已过期、已撤销或不存在 (404)";
    }
    return err.message || `验证失败 (${err.status})`;
  }
  return err instanceof Error ? err.message : "口令验证请求失败";
}

function formatShareTimestamp(raw: string | null | undefined): string {
  if (!raw) return "—";
  const d = new Date(raw);
  if (Number.isNaN(d.getTime())) return raw;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours()
  )}:${pad(d.getMinutes())}`;
}

function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function formatFilterBadgeValue(val: unknown): string {
  if (Array.isArray(val)) {
    return val.map((item) => String(item ?? "")).join(" ~ ");
  }
  if (typeof val === "object" && val !== null) {
    return JSON.stringify(val);
  }
  return String(val ?? "");
}

interface ScenarioImpactRow {
  nodeTitle: string;
  typeName: string;
  metric: string;
  beforeVal: unknown;
  afterVal: unknown;
  delta: unknown;
  deltaRatio: unknown;
}

function extractScenarioImpacts(
  scenario: Record<string, unknown>
): ScenarioImpactRow[] {
  const candidates = [
    scenario.impacts,
    scenario.deltas,
    scenario.nodes,
    isRecord(scenario.result) ? scenario.result.impacts : undefined,
    isRecord(scenario.sim_result) ? scenario.sim_result.impacts : undefined,
  ];
  for (const list of candidates) {
    if (!Array.isArray(list)) continue;
    const rows: ScenarioImpactRow[] = [];
    for (const item of list) {
      if (!isRecord(item)) continue;
      rows.push({
        nodeTitle: String(
          item.title ?? item.node_title ?? item.node_key ?? item.node_id ?? "节点"
        ),
        typeName: String(item.type_name ?? "Entity"),
        metric: String(item.metric ?? item.metric_name ?? "value"),
        beforeVal: item.before_val ?? item.before ?? null,
        afterVal: item.after_val ?? item.after ?? null,
        delta: item.delta ?? null,
        deltaRatio: item.delta_ratio ?? null,
      });
    }
    if (rows.length > 0) return rows;
  }
  return [];
}

export function SharePage() {
  const { token } = useParams<{ token: string }>();

  const [dark, setDark] = useState<boolean>(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  // Strictly in-memory session token (NEVER persisted to localStorage / URL)
  const [shareSession, setShareSession] = useState<string | undefined>(
    undefined
  );
  const [shareData, setShareData] = useState<SharePublicResponse | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [pageError, setPageError] = useState<ShareErrorView | null>(null);

  const [passwordInput, setPasswordInput] = useState<string>("");
  const [unlocking, setUnlocking] = useState<boolean>(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);

  const [downloadFormat, setDownloadFormat] = useState<ExportFormat>("PDF");
  const [downloading, setDownloading] = useState<boolean>(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);

  // Shared dashboard viewer state
  const [filterValues, setFilterValues] = useState<Record<string, unknown>>({});
  const [drillMap, setDrillMap] = useState<Record<string, DrillPathItem[]>>({});
  const [widgetDataMap, setWidgetDataMap] = useState<
    Record<string, WidgetDataResponse | null>
  >({});
  const [loadingMap, setLoadingMap] = useState<Record<string, boolean>>({});
  const [errorMap, setErrorMap] = useState<Record<string, string | null>>({});

  const shareSessionRef = useRef<string | undefined>(undefined);
  const lockedFiltersRef = useRef<Record<string, unknown>>({});

  useEffect(() => {
    shareSessionRef.current = shareSession;
  }, [shareSession]);

  useEffect(() => {
    return () => {
      setShareAssetScope(null);
    };
  }, []);

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    if (typeof document !== "undefined") {
      document.documentElement.classList.toggle("dark", next);
    }
  };

  const fetchSharedWidget = useCallback(
    async (
      shareToken: string,
      widgetId: string,
      dash: DashboardDetail,
      currentFilters: Record<string, unknown>,
      drillPath: DrillPathItem[],
      sessionOverride?: string
    ) => {
      const effectiveFilters = mergeLockedShareFilters(
        currentFilters,
        lockedFiltersRef.current
      );
      const scopedFilters = buildWidgetFilters(
        widgetId,
        dash.filters ?? [],
        effectiveFilters
      );
      const activeSession = sessionOverride ?? shareSessionRef.current;

      setLoadingMap((prev) => ({ ...prev, [widgetId]: true }));
      setErrorMap((prev) => ({ ...prev, [widgetId]: null }));
      try {
        const res = await xblueviewShareApi.querySharedWidgetData(
          shareToken,
          widgetId,
          {
            filters: scopedFilters,
            drill_path: drillPath,
          },
          activeSession
        );
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
    []
  );

  const loadShareMetadata = useCallback(
    async (shareToken: string, sessionOverride?: string) => {
      setLoading(true);
      setPageError(null);
      try {
        const res = await xblueviewShareApi.getShare(
          shareToken,
          sessionOverride
        );
        if (res.password_required) {
          setShareAssetScope(null);
          setShareData(res);
          return;
        }

        // Configure public spatial asset scope BEFORE mounting dashboard child widgets
        setShareAssetScope({
          token: shareToken,
          shareSession: sessionOverride,
        });

        const locked = res.locked_filters ?? {};
        lockedFiltersRef.current = locked;

        if (res.target_kind === "DASHBOARD" && res.dashboard) {
          const defaults: Record<string, unknown> = {};
          for (const f of res.dashboard.filters ?? []) {
            if (f.default !== undefined && f.default !== null) {
              defaults[f.key] = f.default;
            }
          }
          const initialFilters = mergeLockedShareFilters(defaults, locked);
          setFilterValues(initialFilters);
          setDrillMap({});
          setWidgetDataMap({});
          setErrorMap({});
          setShareData(res);

          const nonActionWidgets = res.dashboard.widgets.filter(
            (w) =>
              w.widget_kind !== "ACTION_BUTTON" &&
              w.widget_kind !== "ACTION_FORM"
          );
          for (const w of nonActionWidgets) {
            void fetchSharedWidget(
              shareToken,
              w.id,
              res.dashboard,
              initialFilters,
              [],
              sessionOverride
            );
          }
        } else {
          setShareData(res);
        }
      } catch (err) {
        if (err instanceof ApiError && err.status === 401 && sessionOverride) {
          setShareSession(undefined);
          setShareData({ password_required: true });
          setUnlockError("访问会话已过期，请重新输入口令");
        } else {
          setPageError(classifySharePageError(err));
          setShareData(null);
        }
      } finally {
        setLoading(false);
      }
    },
    [fetchSharedWidget]
  );

  useEffect(() => {
    setShareSession(undefined);
    shareSessionRef.current = undefined;
    setPasswordInput("");
    setUnlockError(null);
    setDownloadError(null);

    if (!token) {
      setLoading(false);
      setPageError({
        status: 404,
        title: "缺少分享令牌 (404)",
        detail: "请检查分享链接是否完整。",
      });
      return;
    }
    void loadShareMetadata(token, undefined);
  }, [loadShareMetadata, token]);

  const handleUnlockSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!token) return;
    const trimmed = passwordInput.trim();
    if (!trimmed) {
      setUnlockError("请输入访问口令");
      return;
    }

    setUnlocking(true);
    setUnlockError(null);
    try {
      const unlockRes = await xblueviewShareApi.unlockShare(token, trimmed);
      setShareSession(unlockRes.share_session);
      shareSessionRef.current = unlockRes.share_session;
      setPasswordInput("");
      await loadShareMetadata(token, unlockRes.share_session);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setPageError(classifySharePageError(err));
        setShareData(null);
      } else {
        setUnlockError(classifyUnlockError(err));
      }
    } finally {
      setUnlocking(false);
    }
  };

  const handleDownload = async () => {
    if (!token || !shareData) return;
    setDownloading(true);
    setDownloadError(null);
    try {
      const isStaticFile = shareData.target_kind === "EXPORT_FILE";
      await xblueviewShareApi.downloadShare(
        token,
        isStaticFile ? undefined : downloadFormat,
        shareSession,
        shareData.export_file?.file_name
      );
    } catch (err) {
      setDownloadError(
        err instanceof Error ? err.message : "下载分享文件失败"
      );
    } finally {
      setDownloading(false);
    }
  };

  const dashboard = shareData?.dashboard ?? null;
  const lockedFilters = shareData?.locked_filters ?? {};
  const lockedEntries = useMemo(
    () => Object.entries(lockedFilters),
    [lockedFilters]
  );
  const lockedKeys = useMemo(
    () => lockedEntries.map(([k]) => k),
    [lockedEntries]
  );

  const handleFilterChange = (key: string, value: unknown) => {
    if (!token || !dashboard || !shareData?.allow_filters) return;
    if (key in lockedFilters) return;

    const next = mergeLockedShareFilters(
      { ...filterValues, [key]: value },
      lockedFilters
    );
    setFilterValues(next);

    const fDef = dashboard.filters.find((f) => f.key === key);
    if (!fDef) return;
    for (const w of dashboard.widgets) {
      if (
        w.widget_kind === "ACTION_BUTTON" ||
        w.widget_kind === "ACTION_FORM"
      ) {
        continue;
      }
      if (fDef.field_map?.[w.id]) {
        void fetchSharedWidget(
          token,
          w.id,
          dashboard,
          next,
          drillMap[w.id] ?? []
        );
      }
    }
  };

  const handleResetFilters = () => {
    if (!token || !dashboard || !shareData?.allow_filters) return;
    const defaults: Record<string, unknown> = {};
    for (const f of dashboard.filters) {
      if (f.default !== undefined && f.default !== null) {
        defaults[f.key] = f.default;
      }
    }
    const resetMerged = mergeLockedShareFilters(defaults, lockedFilters);
    setFilterValues(resetMerged);
    for (const w of dashboard.widgets) {
      if (
        w.widget_kind === "ACTION_BUTTON" ||
        w.widget_kind === "ACTION_FORM"
      ) {
        continue;
      }
      void fetchSharedWidget(
        token,
        w.id,
        dashboard,
        resetMerged,
        drillMap[w.id] ?? []
      );
    }
  };

  const handleDrill = (widgetId: string, field: string, value: unknown) => {
    if (!token || !dashboard) return;
    const widget = dashboard.widgets.find((w) => w.id === widgetId);
    if (!widget || widget.drill.length === 0) return;

    const currentPath = drillMap[widgetId] ?? [];
    const currentLevelDef = widget.drill[currentPath.length];
    if (!currentLevelDef || currentLevelDef.node_type) return;

    if (currentPath.length + 1 < widget.drill.length) {
      const nextPath: DrillPathItem[] = [
        ...currentPath,
        { field: currentLevelDef.field || field, value },
      ];
      setDrillMap((prev) => ({ ...prev, [widgetId]: nextPath }));
      void fetchSharedWidget(token, widgetId, dashboard, filterValues, nextPath);
    }
  };

  const handlePopDrill = (widgetId: string, level: number) => {
    if (!token || !dashboard) return;
    const currentPath = drillMap[widgetId] ?? [];
    const nextPath = currentPath.slice(0, Math.max(0, level));
    setDrillMap((prev) => ({ ...prev, [widgetId]: nextPath }));
    void fetchSharedWidget(token, widgetId, dashboard, filterValues, nextPath);
  };

  const canDownloadInHeader = Boolean(
    shareData &&
      !shareData.password_required &&
      (shareData.target_kind === "EXPORT_FILE" ||
        shareData.allow_export === true)
  );

  const forceDarkCanvas =
    dashboard?.theme === "dark" || dashboard?.theme === "screen";
  const effectiveDark = Boolean(dark || forceDarkCanvas);

  return (
    <div
      className="min-h-screen bg-background text-foreground flex flex-col"
      data-testid="xblueview-share-page"
    >
      {/* Strictly Isolated Public Header (No internal routes / No sidebar) */}
      <header
        className="border-b border-border bg-card px-4 md:px-6 py-3 flex flex-wrap items-center justify-between gap-3 shrink-0"
        data-testid="share-page-header"
      >
        <div className="flex flex-wrap items-center gap-2.5 min-w-0">
          <div className="flex items-center gap-1.5 text-sm font-semibold tracking-tight">
            <span className="inline-block h-2 w-2 rounded-full bg-xblue" />
            <span>xBlueView 安全分享</span>
          </div>
          <Badge variant="xblue" className="gap-1">
            <ShieldCheck className="h-3 w-3" />
            只读视图
          </Badge>
          {dashboard && (
            <span
              className="text-sm font-medium text-foreground truncate"
              data-testid="share-dashboard-title"
            >
              · {dashboard.name}
            </span>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {/* Locked Filter Badges */}
          {lockedEntries.map(([key, val]) => {
            const filterLabel =
              dashboard?.filters.find((f) => f.key === key)?.label ?? key;
            return (
              <Badge
                key={key}
                variant="outline"
                className="gap-1 text-[11px] font-mono-tech"
                data-testid={`share-locked-filter-${key}`}
              >
                <Lock className="h-2.5 w-2.5 text-xblue" />
                {filterLabel}: {formatFilterBadgeValue(val)}
              </Badge>
            );
          })}

          {/* Expiration Badge */}
          {shareData?.expires_at && (
            <Badge
              variant="neutral"
              className="gap-1 font-mono-tech tabular-nums"
              data-testid="share-expires-badge"
            >
              <Clock className="h-3 w-3 text-xblue" />
              到期: {formatShareTimestamp(shareData.expires_at)}
            </Badge>
          )}

          {/* Download Controls */}
          {canDownloadInHeader && (
            <div className="flex items-center gap-1.5">
              {shareData?.target_kind !== "EXPORT_FILE" && (
                <select
                  value={downloadFormat}
                  onChange={(e) =>
                    setDownloadFormat(e.target.value as ExportFormat)
                  }
                  className="h-8 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  data-testid="share-download-format"
                >
                  {SHARE_EXPORT_FORMATS.map((fmt) => (
                    <option key={fmt} value={fmt}>
                      {fmt}
                    </option>
                  ))}
                </select>
              )}
              <Button
                type="button"
                variant="outline"
                disabled={downloading}
                onClick={() => void handleDownload()}
                className="h-8 px-2.5 text-xs"
                data-testid="share-download-btn"
              >
                <Download className="h-3.5 w-3.5 text-xblue" />
                {downloading ? "下载中..." : "下载报告"}
              </Button>
            </div>
          )}

          {/* Theme Toggle */}
          <button
            type="button"
            onClick={toggleTheme}
            className="inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
            data-testid="share-theme-toggle"
          >
            {dark ? (
              <Sun className="h-3.5 w-3.5" />
            ) : (
              <Moon className="h-3.5 w-3.5" />
            )}
            {dark ? "浅色" : "深色"}
          </button>
        </div>
      </header>

      {downloadError && (
        <div className="mx-4 md:mx-6 mt-3 rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
          {downloadError}
        </div>
      )}

      {/* Main Content */}
      <main className="flex-1 w-full max-w-[1600px] mx-auto p-4 md:p-6">
        {loading ? (
          <Card
            className="p-12 text-center text-xs text-muted-foreground"
            data-testid="share-loading-card"
          >
            正在验证并加载安全分享内容...
          </Card>
        ) : pageError ? (
          <div className="max-w-md mx-auto py-16">
            <Card
              className="p-6 space-y-3 text-center"
              data-testid="share-error-card"
            >
              <div className="mx-auto h-10 w-10 rounded-full border border-border bg-muted flex items-center justify-center">
                <AlertTriangle className="h-5 w-5 text-xblue" />
              </div>
              <h1 className="text-base font-semibold">{pageError.title}</h1>
              <p className="text-xs text-muted-foreground leading-relaxed">
                {pageError.detail}
              </p>
            </Card>
          </div>
        ) : shareData?.password_required ? (
          <div className="max-w-md mx-auto py-16">
            <Card
              className="p-6 space-y-4"
              data-testid="share-password-card"
            >
              <div className="flex items-center gap-2.5">
                <div className="h-9 w-9 rounded-md border border-border bg-muted flex items-center justify-center shrink-0">
                  <KeyRound className="h-4 w-4 text-xblue" />
                </div>
                <div>
                  <h1 className="text-sm font-semibold">
                    受口令保护的安全分享
                  </h1>
                  <p className="text-xs text-muted-foreground">
                    请输入分享创建者设置的访问口令以查看内容
                  </p>
                </div>
              </div>

              {unlockError && (
                <div
                  className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
                  data-testid="share-unlock-error"
                >
                  {unlockError}
                </div>
              )}

              <form
                onSubmit={(e) => void handleUnlockSubmit(e)}
                className="space-y-3"
              >
                <Input
                  type="password"
                  value={passwordInput}
                  onChange={(e) => setPasswordInput(e.target.value)}
                  placeholder="输入访问口令"
                  autoFocus
                  className="h-9 text-xs"
                  data-testid="share-unlock-password-input"
                />
                <Button
                  type="submit"
                  disabled={unlocking}
                  className="w-full"
                  data-testid="share-unlock-submit-btn"
                >
                  <Lock className="h-3.5 w-3.5" />
                  {unlocking ? "验证口令中..." : "验证口令并查看"}
                </Button>
              </form>
            </Card>
          </div>
        ) : shareData?.target_kind === "DASHBOARD" && dashboard ? (
          <div
            data-testid="share-dashboard-scope"
            className={
              forceDarkCanvas
                ? "dark bg-zinc-950 text-zinc-100 rounded-lg border border-zinc-800 p-3 space-y-4"
                : "space-y-4"
            }
          >
            {dashboard.description && (
              <p className="text-xs text-muted-foreground">
                {dashboard.description}
              </p>
            )}

            {shareData.allow_filters && dashboard.filters.length > 0 && (
              <FilterBar
                filters={dashboard.filters}
                values={filterValues}
                lockedKeys={lockedKeys}
                shareMode
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
              isEditing={false}
              readOnly
              hideActionWidgets
              isDark={effectiveDark}
              filterValues={filterValues}
              onRefreshWidget={(wid) =>
                token
                  ? void fetchSharedWidget(
                      token,
                      wid,
                      dashboard,
                      filterValues,
                      drillMap[wid] ?? []
                    )
                  : undefined
              }
              onDrill={handleDrill}
              onPopDrill={handlePopDrill}
              onCrossFilter={
                shareData.allow_filters ? handleFilterChange : undefined
              }
            />
          </div>
        ) : shareData?.target_kind === "EXPORT_FILE" &&
          shareData.export_file ? (
          <div className="max-w-lg mx-auto py-12">
            <Card
              className="p-6 space-y-4"
              data-testid="share-export-file-card"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex items-center gap-3 min-w-0">
                  <div className="h-10 w-10 rounded-md border border-border bg-muted flex items-center justify-center shrink-0">
                    <FileText className="h-5 w-5 text-xblue" />
                  </div>
                  <div className="min-w-0">
                    <h1 className="text-sm font-semibold truncate">
                      {shareData.export_file.file_name}
                    </h1>
                    <div className="flex items-center gap-2 mt-1 text-xs font-mono-tech tabular-nums text-muted-foreground">
                      <Badge variant="xblue">
                        {shareData.export_file.export_format}
                      </Badge>
                      <span>{formatBytes(shareData.export_file.file_size)}</span>
                    </div>
                  </div>
                </div>
              </div>

              {shareData.export_file.finished_at && (
                <div className="text-xs font-mono-tech tabular-nums text-muted-foreground border-t border-border pt-3">
                  生成时间: {formatShareTimestamp(shareData.export_file.finished_at)}
                </div>
              )}

              <Button
                type="button"
                disabled={downloading}
                onClick={() => void handleDownload()}
                className="w-full"
                data-testid="share-file-download-btn"
              >
                <Download className="h-4 w-4" />
                {downloading ? "正在下载文件..." : "下载出品文件"}
              </Button>
            </Card>
          </div>
        ) : shareData?.target_kind === "SCENARIO" && shareData.scenario ? (
          <div className="space-y-4" data-testid="share-scenario-card">
            <Card className="p-5 space-y-2">
              <div className="flex items-center gap-2">
                <GitBranch className="h-4 w-4 text-xblue" />
                <h1 className="text-base font-semibold">
                  {String(
                    shareData.scenario.name ??
                      shareData.scenario.title ??
                      "AxSim 因果推演方案"
                  )}
                </h1>
                <Badge variant="xblue">SCENARIO</Badge>
              </div>
              {typeof shareData.scenario.description === "string" &&
                shareData.scenario.description.length > 0 && (
                  <p className="text-xs text-muted-foreground">
                    {shareData.scenario.description}
                  </p>
                )}
            </Card>

            {(() => {
              const impacts = extractScenarioImpacts(shareData.scenario);
              if (impacts.length === 0) {
                return (
                  <Card className="p-4">
                    <pre className="text-xs font-mono-tech overflow-x-auto whitespace-pre-wrap text-muted-foreground">
                      {JSON.stringify(shareData.scenario, null, 2)}
                    </pre>
                  </Card>
                );
              }
              return (
                <Card className="overflow-hidden">
                  <div className="overflow-x-auto">
                    <table className="w-full text-xs border-collapse">
                      <thead>
                        <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                          <th className="px-3 py-2 font-medium">业务节点</th>
                          <th className="px-3 py-2 font-medium">对象类型</th>
                          <th className="px-3 py-2 font-medium">指标</th>
                          <th className="px-3 py-2 font-medium text-right">
                            基线值
                          </th>
                          <th className="px-3 py-2 font-medium text-right">
                            推演值
                          </th>
                          <th className="px-3 py-2 font-medium text-right">
                            变化量
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {impacts.map((row, idx) => (
                          <tr
                            key={`${row.nodeTitle}-${row.metric}-${idx}`}
                            className="border-b border-border/60 last:border-0"
                          >
                            <td className="px-3 py-2 font-medium">
                              {row.nodeTitle}
                            </td>
                            <td className="px-3 py-2 font-mono-tech text-muted-foreground">
                              {row.typeName}
                            </td>
                            <td className="px-3 py-2 font-mono-tech">
                              {row.metric}
                            </td>
                            <td className="px-3 py-2 text-right font-mono-tech tabular-nums">
                              {formatDisplayValue(row.beforeVal)}
                            </td>
                            <td className="px-3 py-2 text-right font-mono-tech tabular-nums font-semibold">
                              {formatDisplayValue(row.afterVal)}
                            </td>
                            <td className="px-3 py-2 text-right font-mono-tech tabular-nums text-xblue">
                              {formatDisplayValue(row.delta)}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </Card>
              );
            })()}
          </div>
        ) : null}
      </main>
    </div>
  );
}
