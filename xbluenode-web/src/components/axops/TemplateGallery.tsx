import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import ReactMarkdown from "react-markdown";
import {
  BookOpen,
  CheckCircle2,
  CopyPlus,
  ExternalLink,
  Eye,
  Layers,
  Package,
  Play,
  RefreshCw,
  Rocket,
  Trash2,
} from "lucide-react";
import {
  axopsApi,
  type DemoLoadResponse,
  type DemoScenario,
  type TemplateConflictStrategy,
  type TemplateInstallRecord,
  type TemplateManifest,
  type TemplatePackDetail,
  type TemplateUninstallResponse,
} from "@/lib/api/axops";
import { xblueApi } from "@/lib/api/xblue";
import { xblueviewApi } from "@/lib/api/xblueview";
import { TemplateHubModal } from "@/components/dashboard/TemplateHubModal";
import {
  Badge,
  Button,
  Card,
  Modal,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import {
  DEMO_SCENARIO_SPECS,
  TEMPLATE_CONFLICT_OPTIONS,
} from "./constants";
import { summarizeTemplatePlanConflicts } from "./logic";

export interface TemplateGalleryProps {
  canManageOps: boolean;
}

export function TemplateGallery({ canManageOps }: TemplateGalleryProps) {
  const navigate = useNavigate();
  const { isZh, tx, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canWrite = canManageOps && can("OPS_ADMIN");
  const canTrialClone = can("STUDIO_EDIT");

  const [templates, setTemplates] = useState<TemplateManifest[]>([]);
  const [installs, setInstalls] = useState<TemplateInstallRecord[]>([]);
  const [existingDashboardSlugs, setExistingDashboardSlugs] = useState<
    Set<string>
  >(new Set());
  const [loading, setLoading] = useState(false);

  // Conflict strategy per pack
  const [strategyByPack, setStrategyByPack] = useState<
    Record<string, TemplateConflictStrategy>
  >({});

  // Active install plan preview
  const [activePlanRecord, setActivePlanRecord] =
    useState<TemplateInstallRecord | null>(null);
  const [planningPackId, setPlanningPackId] = useState<string | null>(null);
  const [applyingInstallId, setApplyingInstallId] = useState<string | null>(
    null
  );
  const [uninstallingId, setUninstallingId] = useState<string | null>(null);
  const [lastUninstallSummary, setLastUninstallSummary] =
    useState<TemplateUninstallResponse | null>(null);

  // README & Topology preview modals + Trial clone state
  const [readmeDetail, setReadmeDetail] = useState<TemplatePackDetail | null>(
    null
  );
  const [readmeLoadingId, setReadmeLoadingId] = useState<string | null>(null);
  const [previewPackId, setPreviewPackId] = useState<string | null>(null);
  const [cloningPackId, setCloningPackId] = useState<string | null>(null);

  // Demo scenario loading
  const [loadingScenario, setLoadingScenario] = useState<DemoScenario | null>(
    null
  );
  const [demoResults, setDemoResults] = useState<
    Partial<Record<DemoScenario, DemoLoadResponse>>
  >({});

  const [noticeMsg, setNoticeMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const [tplRes, instRes, dashRes] = await Promise.all([
        axopsApi.listTemplates(),
        canWrite
          ? axopsApi
              .listTemplateInstalls()
              .catch(() => ({ items: [] as TemplateInstallRecord[] }))
          : Promise.resolve({ items: [] as TemplateInstallRecord[] }),
        xblueviewApi.listDashboards({ limit: 100 }).catch(() => ({
          items: [],
          next_cursor: null,
        })),
      ]);
      setTemplates(Array.isArray(tplRes?.items) ? tplRes.items : []);
      setInstalls(Array.isArray(instRes?.items) ? instRes.items : []);
      const slugs = new Set<string>();
      for (const d of dashRes?.items ?? []) {
        if (d.slug) slugs.add(d.slug);
      }
      setExistingDashboardSlugs(slugs);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("加载模板库失败", "Failed to load templates")
      );
    } finally {
      setLoading(false);
    }
  }, [canWrite, tx]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  const handleOpenReadme = async (packId: string) => {
    setReadmeLoadingId(packId);
    setErrorMsg(null);
    try {
      const detail = await axopsApi.getTemplate(packId);
      setReadmeDetail(detail);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("读取模板说明失败", "Failed to load template README")
      );
    } finally {
      setReadmeLoadingId(null);
    }
  };

  const handleTrialClone = async (packId: string) => {
    if (!canTrialClone || cloningPackId !== null) return;
    setCloningPackId(packId);
    setErrorMsg(null);
    try {
      const res = await xblueApi.cloneSubgraphFromTemplate({ pack_id: packId });
      const targetUrl = res.draft_id
        ? `/studio?subgraph=${encodeURIComponent(res.subgraph_slug)}&draft=${encodeURIComponent(res.draft_id)}`
        : `/studio?subgraph=${encodeURIComponent(res.subgraph_slug)}`;
      void navigate(targetUrl);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("试用克隆失败", "Trial clone failed")
      );
    } finally {
      setCloningPackId(null);
    }
  };

  const handlePlanInstall = async (packId: string) => {
    const strategy = strategyByPack[packId] ?? "RENAME";
    setPlanningPackId(packId);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const planned = await axopsApi.planTemplateInstall(packId, strategy);
      setActivePlanRecord(planned);
      setInstalls((prev) => [
        planned,
        ...prev.filter((item) => item.id !== planned.id),
      ]);
      setNoticeMsg(
        tx(
          `已生成模板包 ${packId} 安装计划（策略: ${strategy}）`,
          `Generated install plan for ${packId} (strategy: ${strategy})`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("生成安装计划失败", "Failed to plan template install")
      );
    } finally {
      setPlanningPackId(null);
    }
  };

  const handleApplyInstall = async (installId: string) => {
    setApplyingInstallId(installId);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const applied = await axopsApi.applyTemplateInstall(installId);
      setInstalls((prev) => {
        const exists = prev.some((i) => i.id === applied.id);
        return exists
          ? prev.map((i) => (i.id === applied.id ? applied : i))
          : [applied, ...prev];
      });
      if (activePlanRecord?.id === applied.id) {
        setActivePlanRecord(applied);
      }
      setNoticeMsg(
        tx(
          `模板安装完成：${applied.pack_id}（已物化 ${Object.keys(applied.object_map ?? {}).length} 个对象）`,
          `Installed ${applied.pack_id} (${Object.keys(applied.object_map ?? {}).length} objects materialized)`
        )
      );
      await loadAll();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? tx(`安装中断：${err.message}`, `Install interrupted: ${err.message}`)
          : tx("执行模板安装失败", "Template install failed")
      );
      await loadAll();
    } finally {
      setApplyingInstallId(null);
    }
  };

  const handleUninstall = async (installId: string) => {
    setUninstallingId(installId);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const res = await axopsApi.uninstallTemplate(installId);
      setLastUninstallSummary(res);
      setNoticeMsg(
        tx(
          `卸载完成：移除未改动对象 ${res.deleted.length} 个，保留已修改对象 ${res.kept.length} 个`,
          `Uninstalled: removed ${res.deleted.length} untouched objects, preserved ${res.kept.length} modified objects`
        )
      );
      await loadAll();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("卸载模板失败", "Failed to uninstall template")
      );
    } finally {
      setUninstallingId(null);
    }
  };

  const handleLoadDemo = async (scenario: DemoScenario) => {
    setLoadingScenario(scenario);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const res = await axopsApi.loadDemoScenario(scenario);
      setDemoResults((prev) => ({ ...prev, [scenario]: res }));
      if (res.dashboard_slug) {
        setExistingDashboardSlugs((prev) => {
          const next = new Set(prev);
          next.add(res.dashboard_slug);
          return next;
        });
      }
      setNoticeMsg(
        tx(
          `演示场景 ${scenario} 装载完成：写入 ${res.metric_points_written} 条时序点`,
          `Demo scenario ${scenario} loaded: ${res.metric_points_written} metric points written`
        )
      );
      await loadAll();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("装载演示场景失败", "Failed to load demo scenario")
      );
    } finally {
      setLoadingScenario(null);
    }
  };

  const planSummary = summarizeTemplatePlanConflicts(activePlanRecord?.plan);

  return (
    <div className="space-y-5" data-testid="axops-templates-panel">
      {errorMsg && (
        <div
          role="alert"
          className="rounded-md border border-red-500/40 bg-red-500/10 px-3.5 py-2 text-xs text-red-700 dark:text-red-300"
        >
          {errorMsg}
        </div>
      )}

      {noticeMsg && (
        <div
          data-testid="axops-templates-notice"
          className="rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2 text-xs text-xblue"
        >
          {noticeMsg}
        </div>
      )}

      {/* 1. One-Click Demo Scenarios */}
      <Card className="p-4 space-y-3" data-testid="demo-scenarios-section">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
          <div className="space-y-0.5">
            <div className="text-sm font-semibold flex items-center gap-2">
              <Rocket className="h-4 w-4 text-xblue" />
              {tx("全栈演示场景", "Full-Stack Demo Scenarios")}
            </div>
            <p className="text-xs text-muted-foreground">
              {tx(
                "自动安装行业模板包、刷新业务数据、写入 30 日历史时序回放并生成推演沙盘与驾驶舱。",
                "Installs template packs, syncs datasets, replays 30-day telemetry, and provisions simulation scenarios and dashboards."
              )}
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={loading}
            onClick={() => void loadAll()}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            {tx("刷新", "Refresh")}
          </Button>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          {DEMO_SCENARIO_SPECS.map((spec) => {
            const loaded = demoResults[spec.scenario];
            const isRunning = loadingScenario === spec.scenario;
            const targetSlug =
              loaded?.dashboard_slug ?? spec.defaultDashboardSlug;
            const isDashboardReady =
              Boolean(loaded?.dashboard_slug) ||
              existingDashboardSlugs.has(targetSlug);

            return (
              <Card
                key={spec.scenario}
                data-testid={`demo-card-${spec.scenario}`}
                className="p-3.5 flex flex-col justify-between gap-3 bg-muted/15"
              >
                <div className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs font-semibold text-foreground">
                      {isZh ? spec.title : spec.enTitle}
                    </span>
                    <Badge variant="outline">
                      {isZh ? spec.industry : spec.enIndustry}
                    </Badge>
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {spec.nodeTypes.map((nt) => (
                      <Badge key={nt} variant="neutral" className="text-[10px]">
                        {nt}
                      </Badge>
                    ))}
                  </div>
                  <p className="text-[11px] text-muted-foreground leading-relaxed">
                    {isZh ? spec.highlights : spec.enHighlights}
                  </p>
                </div>

                <div className="space-y-2 pt-2 border-t border-border/70">
                  {loaded && (
                    <div
                      data-testid={`demo-loaded-summary-${spec.scenario}`}
                      className="rounded border border-xblue/35 bg-xblue-soft/60 p-2 text-[11px] space-y-1"
                    >
                      <div className="font-mono-tech text-xblue font-medium">
                        {tx(
                          `时序写入: ${fmtNum(loaded.metric_points_written, 0)} 点`,
                          `Telemetry: ${fmtNum(loaded.metric_points_written, 0)} pts`
                        )}
                      </div>
                      <Link
                        to={`/view/${encodeURIComponent(loaded.dashboard_slug)}`}
                        data-testid={`demo-dashboard-link-${spec.scenario}`}
                        className="inline-flex items-center gap-1 font-medium text-xblue underline"
                      >
                        {tx("打开驾驶舱", "Open Dashboard")} (/view/
                        {loaded.dashboard_slug})
                        <ExternalLink className="h-3 w-3" />
                      </Link>
                    </div>
                  )}

                  <div className="flex items-center justify-between gap-2">
                    {isDashboardReady ? (
                      <Link
                        to={`/view/${encodeURIComponent(targetSlug)}`}
                        className="text-[11px] font-mono-tech text-xblue hover:underline inline-flex items-center gap-1"
                      >
                        /view/{targetSlug}
                        <ExternalLink className="h-2.5 w-2.5" />
                      </Link>
                    ) : (
                      <Badge
                        variant="outline"
                        data-testid={`demo-dashboard-unavailable-${spec.scenario}`}
                      >
                        {tx("装载后可用", "Install to enable")}
                      </Badge>
                    )}

                    {canWrite && (
                      <Button
                        type="button"
                        variant="primary"
                        size="sm"
                        data-testid={`load-demo-btn-${spec.scenario}`}
                        disabled={loadingScenario !== null}
                        onClick={() => void handleLoadDemo(spec.scenario)}
                      >
                        <Play className="h-3 w-3" />
                        {isRunning
                          ? tx("装载中…", "Loading…")
                          : tx("装载演示", "Load Demo")}
                      </Button>
                    )}
                  </div>
                </div>
              </Card>
            );
          })}
        </div>
      </Card>

      {/* 2. Industry Template Packs */}
      <div className="space-y-3">
        <div className="flex items-center gap-2 text-sm font-semibold">
          <Package className="h-4 w-4 text-xblue" />
          <span>{tx("行业标准模板包", "Industry Template Packs")}</span>
          <Badge variant="outline">{fmtNum(templates.length, 0)}</Badge>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          {templates.map((pack) => {
            const strategy = strategyByPack[pack.id] ?? "RENAME";
            const isPlanning = planningPackId === pack.id;
            const shaCount = Object.keys(pack.sha256 ?? {}).length;

            return (
              <Card
                key={pack.id}
                data-testid={`template-pack-card-${pack.id}`}
                className="p-4 flex flex-col justify-between gap-3"
              >
                <div className="space-y-2">
                  <div className="flex items-start justify-between gap-2">
                    <div>
                      <h3 className="text-sm font-semibold">{pack.name}</h3>
                      <div className="font-mono-tech text-[11px] text-muted-foreground">
                        {pack.id} · v{pack.version}
                      </div>
                    </div>
                    <Badge variant="xblue">{pack.industry}</Badge>
                  </div>

                  <p className="text-xs text-muted-foreground leading-relaxed">
                    {pack.description}
                  </p>

                  <div className="flex flex-wrap gap-1.5 pt-1">
                    <Badge variant="outline">
                      {tx("数据集", "Datasets")} ×
                      {pack.contents?.datasets?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      {tx("流程", "Flows")} ×{pack.contents?.flows?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      {tx("驾驶舱", "Dashboards")} ×
                      {pack.contents?.dashboards?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      {tx("智能体", "Agents")} ×
                      {pack.contents?.agents?.length ?? 0}
                    </Badge>
                    <Badge variant="neutral">SHA-256 ×{shaCount}</Badge>
                  </div>
                </div>

                <div className="space-y-2 pt-2.5 border-t border-border">
                  {canWrite && (
                    <div className="flex items-center gap-2">
                      <label className="text-[11px] text-muted-foreground shrink-0">
                        {tx("冲突策略", "Conflict")}
                      </label>
                      <Select
                        data-testid={`template-conflict-select-${pack.id}`}
                        value={strategy}
                        onChange={(e) =>
                          setStrategyByPack((prev) => ({
                            ...prev,
                            [pack.id]: e.target
                              .value as TemplateConflictStrategy,
                          }))
                        }
                        className="h-7 flex-1 text-[11px]"
                      >
                        {TEMPLATE_CONFLICT_OPTIONS.map((opt) => (
                          <option key={opt.value} value={opt.value}>
                            {isZh ? opt.label : opt.enLabel}
                          </option>
                        ))}
                      </Select>
                    </div>
                  )}

                  <div className="flex flex-wrap items-center justify-between gap-1.5">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        data-testid={`preview-template-btn-${pack.id}`}
                        onClick={() => setPreviewPackId(pack.id)}
                      >
                        <Eye className="h-3.5 w-3.5 text-xblue" />
                        {tx("预览模板", "Preview")}
                      </Button>

                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        data-testid={`open-template-readme-btn-${pack.id}`}
                        disabled={readmeLoadingId === pack.id}
                        onClick={() => void handleOpenReadme(pack.id)}
                      >
                        <BookOpen className="h-3.5 w-3.5 text-xblue" />
                        {readmeLoadingId === pack.id
                          ? tx("读取中…", "Loading…")
                          : tx("说明文档", "README")}
                      </Button>
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5">
                      {canTrialClone && (
                        <Button
                          type="button"
                          variant={canWrite ? "outline" : "primary"}
                          size="sm"
                          data-testid={`trial-clone-template-btn-${pack.id}`}
                          disabled={cloningPackId !== null}
                          onClick={() => void handleTrialClone(pack.id)}
                        >
                          <CopyPlus className="h-3.5 w-3.5" />
                          {cloningPackId === pack.id
                            ? tx("克隆中…", "Cloning…")
                            : tx("试用克隆", "Try Clone")}
                        </Button>
                      )}

                      {canWrite && (
                        <Button
                          type="button"
                          variant="primary"
                          size="sm"
                          data-testid={`plan-template-btn-${pack.id}`}
                          disabled={isPlanning}
                          onClick={() => void handlePlanInstall(pack.id)}
                        >
                          <Layers className="h-3.5 w-3.5" />
                          {isPlanning
                            ? tx("规划中…", "Planning…")
                            : tx("规划安装", "Plan Install")}
                        </Button>
                      )}
                    </div>
                  </div>
                </div>
              </Card>
            );
          })}
        </div>
      </div>

      {/* 3. Step-2 Install Plan Preview Card */}
      {activePlanRecord && (
        <Card
          className="p-4 space-y-3 border-xblue/50"
          data-testid="template-plan-preview-card"
        >
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-semibold">
                {tx("安装计划", "Install Plan")}: {activePlanRecord.pack_id} (v
                {activePlanRecord.pack_version})
              </span>
              <StatusBadge status={activePlanRecord.status} />
              <Badge variant="outline">
                {activePlanRecord.plan.on_conflict}
              </Badge>
              <Badge variant="outline">
                {tx(
                  `对象 ${planSummary.totalObjects} · 冲突 ${planSummary.conflictCount} · 重命名 ${planSummary.renameCount} · 跳过 ${planSummary.skipCount}`,
                  `Objects ${planSummary.totalObjects} · Conflicts ${planSummary.conflictCount} · Rename ${planSummary.renameCount} · Skip ${planSummary.skipCount}`
                )}
              </Badge>
            </div>

            <div className="flex items-center gap-2">
              {canWrite &&
                (activePlanRecord.status === "PLANNED" ||
                  activePlanRecord.status === "FAILED") && (
                  <Button
                    type="button"
                    variant="primary"
                    size="sm"
                    data-testid="apply-template-install-btn"
                    disabled={applyingInstallId === activePlanRecord.id}
                    onClick={() => void handleApplyInstall(activePlanRecord.id)}
                  >
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    {applyingInstallId === activePlanRecord.id
                      ? tx("安装中…", "Installing…")
                      : activePlanRecord.status === "FAILED"
                        ? tx("断点续装", "Resume")
                        : tx("执行安装", "Install")}
                  </Button>
                )}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setActivePlanRecord(null)}
              >
                {tx("收起", "Collapse")}
              </Button>
            </div>
          </div>

          <div className="overflow-x-auto max-h-60">
            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-border text-[11px] text-muted-foreground">
                  <th className="py-1.5 pr-3">{tx("类别", "Category")}</th>
                  <th className="py-1.5 pr-3">{tx("键名", "Key")}</th>
                  <th className="py-1.5 pr-3">{tx("源名称", "Source")}</th>
                  <th className="py-1.5 pr-3">{tx("目标名称", "Target")}</th>
                  <th className="py-1.5">{tx("动作", "Action")}</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border/60 font-mono-tech text-[11px]">
                {(activePlanRecord.plan.objects ?? []).map((obj, idx) => (
                  <tr key={`${obj.category}-${obj.key}-${idx}`}>
                    <td className="py-1.5 pr-3">{obj.category}</td>
                    <td className="py-1.5 pr-3">{obj.key}</td>
                    <td className="py-1.5 pr-3">{obj.source_name}</td>
                    <td className="py-1.5 pr-3 font-semibold text-foreground">
                      {obj.target_name}
                    </td>
                    <td className="py-1.5">
                      <Badge variant={obj.conflict ? "warning" : "neutral"}>
                        {obj.action}
                      </Badge>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}

      {/* Last Uninstall Summary */}
      {lastUninstallSummary && (
        <Card
          className="p-3.5 flex flex-wrap items-center justify-between gap-2 text-xs"
          data-testid="template-uninstall-summary"
        >
          <div>
            <span className="font-semibold">
              {tx("安全卸载结果：", "Safe Uninstall Summary: ")}
            </span>
            {tx(
              `移除未改动对象 ${lastUninstallSummary.deleted.length} 项，保留已修改对象 ${lastUninstallSummary.kept.length} 项。`,
              `Removed ${lastUninstallSummary.deleted.length} untouched objects; preserved ${lastUninstallSummary.kept.length} user-modified objects.`
            )}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setLastUninstallSummary(null)}
          >
            {tx("关闭", "Close")}
          </Button>
        </Card>
      )}

      {/* 4. Installed Packs Table */}
      <Card
        className="overflow-hidden"
        data-testid="template-installs-table-card"
      >
        <div className="px-4 py-3 border-b border-border bg-muted/30 flex items-center justify-between">
          <span className="text-xs font-semibold">
            {tx("模板安装记录", "Template Installations")}
          </span>
          <Badge variant="outline">{fmtNum(installs.length, 0)}</Badge>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/20 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("模板包 / 版本", "Pack / Version")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("状态", "Status")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("冲突策略", "Conflict")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("物化对象", "Objects")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("创建时间", "Created")}
                </th>
                <th className="py-2.5 px-3 text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {installs.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-8 text-center text-muted-foreground"
                  >
                    {tx("暂无安装记录", "No template installations")}
                  </td>
                </tr>
              ) : (
                installs.map((inst) => {
                  const createdCount = Object.keys(
                    inst.object_map ?? {}
                  ).length;
                  const canApply =
                    canWrite &&
                    (inst.status === "PLANNED" || inst.status === "FAILED");
                  const canUninstall =
                    canWrite &&
                    (inst.status === "INSTALLED" ||
                      (inst.status === "FAILED" && createdCount > 0));

                  return (
                    <tr
                      key={inst.id}
                      data-testid={`template-install-row-${inst.id}`}
                      className="hover:bg-muted/30"
                    >
                      <td className="py-2.5 px-3">
                        <div className="font-medium font-mono-tech">
                          {inst.pack_id} · v{inst.pack_version}
                        </div>
                        <div className="font-mono-tech text-[11px] text-muted-foreground">
                          {inst.id.slice(0, 8)}…
                        </div>
                        {inst.error_message && (
                          <div className="text-[11px] text-red-600 dark:text-red-400 mt-0.5">
                            {inst.error_message}
                          </div>
                        )}
                      </td>
                      <td className="py-2.5 px-3 whitespace-nowrap">
                        <StatusBadge status={inst.status} />
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech">
                        {inst.plan?.on_conflict ?? "RENAME"}
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech">
                        {fmtNum(createdCount, 0)}
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                        {fmtTime(inst.created_at, true)}
                      </td>
                      <td className="py-2.5 px-3 text-right whitespace-nowrap">
                        <div className="inline-flex items-center gap-1.5">
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() => setActivePlanRecord(inst)}
                          >
                            {tx("计划详情", "Plan")}
                          </Button>
                          {canApply && (
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              data-testid={`apply-install-row-btn-${inst.id}`}
                              disabled={applyingInstallId === inst.id}
                              onClick={() => void handleApplyInstall(inst.id)}
                            >
                              {inst.status === "FAILED"
                                ? tx("断点续装", "Resume")
                                : tx("执行安装", "Install")}
                            </Button>
                          )}
                          {canUninstall && (
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              data-testid={`uninstall-template-btn-${inst.id}`}
                              disabled={uninstallingId === inst.id}
                              onClick={() => void handleUninstall(inst.id)}
                            >
                              <Trash2 className="h-3 w-3" />
                              {tx("卸载", "Uninstall")}
                            </Button>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </Card>

      {/* README Preview Modal */}
      <Modal
        open={Boolean(readmeDetail)}
        onClose={() => setReadmeDetail(null)}
        testId="template-readme-modal"
        maxWidthClass="max-w-2xl"
        title={
          readmeDetail ? (
            <>
              <BookOpen className="h-4 w-4 text-xblue shrink-0" />
              <span className="truncate">{readmeDetail.name}</span>
              <Badge variant="outline">v{readmeDetail.version}</Badge>
            </>
          ) : (
            tx("模板说明", "Template README")
          )
        }
      >
        {readmeDetail && (
          <div className="text-xs leading-relaxed space-y-2 prose prose-sm dark:prose-invert max-w-none">
            <ReactMarkdown>
              {readmeDetail.readme ||
                `# ${readmeDetail.name}\n\n${readmeDetail.description}`}
            </ReactMarkdown>
          </div>
        )}
      </Modal>

      <TemplateHubModal
        open={Boolean(previewPackId)}
        initialPackId={previewPackId}
        onClose={() => setPreviewPackId(null)}
      />
    </div>
  );
}
