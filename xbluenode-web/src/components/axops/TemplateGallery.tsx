import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router";
import ReactMarkdown from "react-markdown";
import {
  BookOpen,
  CheckCircle2,
  ExternalLink,
  Layers,
  Package,
  Play,
  RefreshCw,
  Rocket,
  Trash2,
  X,
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
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  DEMO_SCENARIO_SPECS,
  TEMPLATE_CONFLICT_OPTIONS,
  TEMPLATE_INSTALL_STATUS_LABELS,
} from "./constants";
import { summarizeTemplatePlanConflicts } from "./logic";

export interface TemplateGalleryProps {
  canManageOps: boolean;
}

export function TemplateGallery({ canManageOps }: TemplateGalleryProps) {
  const [templates, setTemplates] = useState<TemplateManifest[]>([]);
  const [installs, setInstalls] = useState<TemplateInstallRecord[]>([]);
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

  // README preview modal
  const [readmeDetail, setReadmeDetail] = useState<TemplatePackDetail | null>(
    null
  );
  const [readmeLoadingId, setReadmeLoadingId] = useState<string | null>(null);

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
      const [tplRes, instRes] = await Promise.all([
        axopsApi.listTemplates(),
        axopsApi.listTemplateInstalls(),
      ]);
      setTemplates(Array.isArray(tplRes?.items) ? tplRes.items : []);
      setInstalls(Array.isArray(instRes?.items) ? instRes.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "加载模板库数据失败");
    } finally {
      setLoading(false);
    }
  }, []);

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
      setErrorMsg(err instanceof Error ? err.message : "读取模板说明失败");
    } finally {
      setReadmeLoadingId(null);
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
        `已生成模板包 ${packId} 安装计划（冲突策略: ${strategy}），请核对下方对象清单后点击「执行安装」`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "生成安装计划失败");
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
        `模板安装执行完成：${applied.pack_id} · 状态 ${applied.status}（已创建对象 ${Object.keys(applied.object_map ?? {}).length} 个）`
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? `安装中断（可点击断点续装恢复）: ${err.message}`
          : "执行模板安装失败"
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
        `安全卸载完成：删除未改动对象 ${res.deleted.length} 个，保留用户已修改对象 ${res.kept.length} 个`
      );
      await loadAll();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "安全卸载模板失败");
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
      setNoticeMsg(
        `演示场景 ${scenario} 装载完成：已写入 ${res.metric_points_written} 条 30 日回放时序点，驾驶舱 /view/${res.dashboard_slug} 已就绪`
      );
      await loadAll();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "装载演示场景失败");
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
              一键装载全栈演示场景 (Demo Scenarios)
            </div>
            <p className="text-xs text-muted-foreground">
              自动安装对应行业模板包、执行首次数据刷新流程、生成 30 日历史时序回放，并预置因果推演沙盘与驾驶舱。
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            className="h-8 px-2.5 text-xs"
            disabled={loading}
            onClick={() => void loadAll()}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            刷新状态
          </Button>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          {DEMO_SCENARIO_SPECS.map((spec) => {
            const loaded = demoResults[spec.scenario];
            const isRunning = loadingScenario === spec.scenario;
            return (
              <Card
                key={spec.scenario}
                data-testid={`demo-card-${spec.scenario}`}
                className="p-3.5 flex flex-col justify-between gap-3 bg-muted/15"
              >
                <div className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs font-semibold text-foreground">
                      {spec.title}
                    </span>
                    <Badge variant="outline">{spec.industry}</Badge>
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {spec.nodeTypes.map((nt) => (
                      <Badge key={nt} variant="neutral" className="text-[10px]">
                        {nt}
                      </Badge>
                    ))}
                  </div>
                  <p className="text-[11px] text-muted-foreground leading-relaxed">
                    {spec.highlights}
                  </p>
                </div>

                <div className="space-y-2 pt-2 border-t border-border/70">
                  {loaded && (
                    <div
                      data-testid={`demo-loaded-summary-${spec.scenario}`}
                      className="rounded border border-xblue/35 bg-xblue-soft/60 p-2 text-[11px] space-y-1"
                    >
                      <div className="font-mono-tech text-xblue font-medium">
                        时序写入: {loaded.metric_points_written} 点
                      </div>
                      <Link
                        to={`/view/${encodeURIComponent(loaded.dashboard_slug)}`}
                        data-testid={`demo-dashboard-link-${spec.scenario}`}
                        className="inline-flex items-center gap-1 font-medium text-xblue underline"
                      >
                        打开驾驶舱 (/view/{loaded.dashboard_slug})
                        <ExternalLink className="h-3 w-3" />
                      </Link>
                    </div>
                  )}

                  <div className="flex items-center justify-between gap-2">
                    <Link
                      to={`/view/${encodeURIComponent(
                        loaded?.dashboard_slug ?? spec.defaultDashboardSlug
                      )}`}
                      className="text-[11px] font-mono-tech text-muted-foreground hover:text-xblue inline-flex items-center gap-1"
                    >
                      /view/{loaded?.dashboard_slug ?? spec.defaultDashboardSlug}
                      <ExternalLink className="h-2.5 w-2.5" />
                    </Link>

                    {canManageOps && (
                      <Button
                        type="button"
                        variant="primary"
                        className="h-7 px-2.5 text-xs"
                        data-testid={`load-demo-btn-${spec.scenario}`}
                        disabled={loadingScenario !== null}
                        onClick={() => void handleLoadDemo(spec.scenario)}
                      >
                        <Play className="h-3 w-3" />
                        {isRunning ? "装载中..." : "一键装载演示"}
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
          <span>行业标准模板包 (SHA-256 完整性校验 · 两步冲突感知安装)</span>
          <Badge variant="outline">{templates.length} 套</Badge>
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
                      数据集 ×{pack.contents?.datasets?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      流程 ×{pack.contents?.flows?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      驾驶舱 ×{pack.contents?.dashboards?.length ?? 0}
                    </Badge>
                    <Badge variant="outline">
                      智能体 ×{pack.contents?.agents?.length ?? 0}
                    </Badge>
                    <Badge variant="neutral">SHA-256 ×{shaCount}</Badge>
                  </div>
                </div>

                <div className="space-y-2 pt-2.5 border-t border-border">
                  <div className="flex items-center gap-2">
                    <label className="text-[11px] text-muted-foreground shrink-0">
                      冲突策略:
                    </label>
                    <select
                      data-testid={`template-conflict-select-${pack.id}`}
                      value={strategy}
                      onChange={(e) =>
                        setStrategyByPack((prev) => ({
                          ...prev,
                          [pack.id]: e.target.value as TemplateConflictStrategy,
                        }))
                      }
                      className="h-7 flex-1 rounded border border-input bg-background px-2 text-[11px]"
                    >
                      {TEMPLATE_CONFLICT_OPTIONS.map((opt) => (
                        <option key={opt.value} value={opt.value}>
                          {opt.label}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="flex items-center justify-between gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      className="h-8 px-2.5 text-xs"
                      data-testid={`open-template-readme-btn-${pack.id}`}
                      disabled={readmeLoadingId === pack.id}
                      onClick={() => void handleOpenReadme(pack.id)}
                    >
                      <BookOpen className="h-3.5 w-3.5 text-xblue" />
                      {readmeLoadingId === pack.id ? "读取中..." : "查看说明"}
                    </Button>

                    <Button
                      type="button"
                      variant="primary"
                      className="h-8 px-3 text-xs"
                      data-testid={`plan-template-btn-${pack.id}`}
                      disabled={isPlanning}
                      onClick={() => void handlePlanInstall(pack.id)}
                    >
                      <Layers className="h-3.5 w-3.5" />
                      {isPlanning ? "规划中..." : "生成安装计划"}
                    </Button>
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
                安装计划预览：{activePlanRecord.pack_id} (v
                {activePlanRecord.pack_version})
              </span>
              <Badge variant="xblue">
                {TEMPLATE_INSTALL_STATUS_LABELS[activePlanRecord.status] ??
                  activePlanRecord.status}
              </Badge>
              <Badge variant="outline">
                策略: {activePlanRecord.plan.on_conflict}
              </Badge>
              <Badge variant="outline">
                总对象: {planSummary.totalObjects} · 冲突:{" "}
                {planSummary.conflictCount} · 重命名: {planSummary.renameCount} ·
                跳过: {planSummary.skipCount}
              </Badge>
            </div>

            <div className="flex items-center gap-2">
              {canManageOps &&
                (activePlanRecord.status === "PLANNED" ||
                  activePlanRecord.status === "FAILED") && (
                  <Button
                    type="button"
                    variant="primary"
                    className="h-8 px-3 text-xs"
                    data-testid="apply-template-install-btn"
                    disabled={applyingInstallId === activePlanRecord.id}
                    onClick={() => void handleApplyInstall(activePlanRecord.id)}
                  >
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    {applyingInstallId === activePlanRecord.id
                      ? "安装执行中..."
                      : activePlanRecord.status === "FAILED"
                        ? "断点续装"
                        : "确认执行安装"}
                  </Button>
                )}
              <Button
                type="button"
                variant="ghost"
                className="h-8 px-2 text-xs"
                onClick={() => setActivePlanRecord(null)}
              >
                收起
              </Button>
            </div>
          </div>

          <div className="overflow-x-auto max-h-60">
            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-border text-[11px] text-muted-foreground">
                  <th className="py-1.5 pr-3">类别</th>
                  <th className="py-1.5 pr-3">模板键</th>
                  <th className="py-1.5 pr-3">源名称</th>
                  <th className="py-1.5 pr-3">目标名称</th>
                  <th className="py-1.5">冲突与动作</th>
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
                      <Badge variant={obj.conflict ? "xblue" : "neutral"}>
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
            <span className="font-semibold">安全卸载哈希核验结果：</span>{" "}
            已移除未改动对象{" "}
            <span className="font-mono-tech font-semibold text-xblue">
              {lastUninstallSummary.deleted.length}
            </span>{" "}
            项，保留用户自定义改动对象{" "}
            <span className="font-mono-tech font-semibold">
              {lastUninstallSummary.kept.length}
            </span>{" "}
            项。
          </div>
          <Button
            type="button"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={() => setLastUninstallSummary(null)}
          >
            关闭
          </Button>
        </Card>
      )}

      {/* 4. Installed Packs Table */}
      <Card className="overflow-hidden" data-testid="template-installs-table-card">
        <div className="px-4 py-3 border-b border-border bg-muted/30 flex items-center justify-between">
          <span className="text-xs font-semibold">
            模板安装记录与生命周期管控 (ax_template_installs)
          </span>
          <Badge variant="outline">{installs.length} 条记录</Badge>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/20 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3">模板包 / 版本</th>
                <th className="py-2.5 px-3">状态</th>
                <th className="py-2.5 px-3">冲突策略</th>
                <th className="py-2.5 px-3">已物化对象数</th>
                <th className="py-2.5 px-3">创建时间</th>
                <th className="py-2.5 px-3 text-right">操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {installs.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="py-8 text-center text-muted-foreground"
                  >
                    暂无模板安装记录
                  </td>
                </tr>
              ) : (
                installs.map((inst) => {
                  const createdCount = Object.keys(inst.object_map ?? {}).length;
                  const canApply =
                    canManageOps &&
                    (inst.status === "PLANNED" || inst.status === "FAILED");
                  const canUninstall =
                    canManageOps &&
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
                          {inst.id}
                        </div>
                        {inst.error_message && (
                          <div className="text-[11px] text-red-600 dark:text-red-400 mt-0.5">
                            {inst.error_message}
                          </div>
                        )}
                      </td>
                      <td className="py-2.5 px-3">
                        <Badge
                          variant={
                            inst.status === "INSTALLED" ? "xblue" : "outline"
                          }
                        >
                          {TEMPLATE_INSTALL_STATUS_LABELS[inst.status] ??
                            inst.status}
                        </Badge>
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech">
                        {inst.plan?.on_conflict ?? "RENAME"}
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech">
                        {createdCount} 项
                      </td>
                      <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground">
                        {inst.created_at}
                      </td>
                      <td className="py-2.5 px-3 text-right">
                        <div className="inline-flex items-center gap-1.5">
                          <Button
                            type="button"
                            variant="ghost"
                            className="h-7 px-2 text-[11px]"
                            onClick={() => setActivePlanRecord(inst)}
                          >
                            查看计划
                          </Button>
                          {canApply && (
                            <Button
                              type="button"
                              variant="outline"
                              className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue"
                              data-testid={`apply-install-row-btn-${inst.id}`}
                              disabled={applyingInstallId === inst.id}
                              onClick={() => void handleApplyInstall(inst.id)}
                            >
                              {inst.status === "FAILED" ? "断点续装" : "执行安装"}
                            </Button>
                          )}
                          {canUninstall && (
                            <Button
                              type="button"
                              variant="outline"
                              className="h-7 px-2.5 text-[11px]"
                              data-testid={`uninstall-template-btn-${inst.id}`}
                              disabled={uninstallingId === inst.id}
                              onClick={() => void handleUninstall(inst.id)}
                            >
                              <Trash2 className="h-3 w-3" />
                              安全卸载
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
      {readmeDetail && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
          data-testid="template-readme-modal"
          onClick={() => setReadmeDetail(null)}
        >
          <Card
            className="w-full max-w-2xl max-h-[85vh] flex flex-col overflow-hidden bg-background shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 border-b border-border flex items-center justify-between">
              <div className="flex items-center gap-2">
                <BookOpen className="h-4 w-4 text-xblue" />
                <span className="text-sm font-semibold">
                  {readmeDetail.name} · 模板包业务说明 (README.md)
                </span>
                <Badge variant="outline">v{readmeDetail.version}</Badge>
              </div>
              <button
                type="button"
                onClick={() => setReadmeDetail(null)}
                className="rounded p-1 text-muted-foreground hover:text-foreground cursor-pointer"
                aria-label="关闭模板说明"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-5 text-xs leading-relaxed space-y-2 prose prose-sm dark:prose-invert max-w-none">
              <ReactMarkdown>
                {readmeDetail.readme || `# ${readmeDetail.name}\n\n${readmeDetail.description}`}
              </ReactMarkdown>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
