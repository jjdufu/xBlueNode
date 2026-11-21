import { useCallback, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import {
  Activity,
  AlertTriangle,
  Bell,
  BellOff,
  CheckCircle2,
  ExternalLink,
  Kanban,
  Send,
  ShieldCheck,
  SlidersHorizontal,
} from "lucide-react";
import {
  ackAlert,
  ackAlertsBatch,
  axflowApi,
  createAlertSilence,
  createNotifyChannel,
  createRule,
  deleteAlertSilence,
  deleteNotifyChannel,
  deleteRule,
  dryRunAdhocRule,
  dryRunSavedRule,
  listAlertSilences,
  listAlerts,
  listNotifyChannels,
  listRules,
  resolveAlert,
  testNotifyChannel,
  updateNotifyChannel,
  updateRule,
  xblueApi,
  type AlertListQuery,
  type AlertRecord,
  type AlertSilenceRecord,
  type CreateNotifyChannelRequest,
  type CreateRuleRequest,
  type CreateSilenceRequest,
  type FlowRecord,
  type NotifyChannelRecord,
  type RuleDef,
  type RuleDryRunResponse,
  type TestChannelResponse,
  type UpdateNotifyChannelRequest,
  type UpdateRuleRequest,
} from "@/lib/api";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import { axopsApi } from "@/lib/api/axops";
import { AlertDetail } from "@/components/axalerts/AlertDetail";
import { AlertTable } from "@/components/axalerts/AlertTable";
import { ChannelForm } from "@/components/axalerts/ChannelForm";
import { RuleEditor } from "@/components/axalerts/RuleEditor";
import { SilenceForm } from "@/components/axalerts/SilenceForm";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

type AlertsTabKey = "alerts" | "rules" | "channels" | "silences";

function formatErrorMessage(err: unknown, fallback: string): string {
  if (err instanceof Error && err.message.trim().length > 0) {
    return err.message;
  }
  return fallback;
}

function isSilenceActiveNow(silence: AlertSilenceRecord): boolean {
  const now = Date.now();
  const start = new Date(silence.starts_at).getTime();
  const end = new Date(silence.ends_at).getTime();
  if (Number.isFinite(start) && Number.isFinite(end)) {
    return now >= start && now < end;
  }
  return true;
}

export function AxAlertsPage() {
  const queryClient = useQueryClient();

  const [activeTab, setActiveTab] = useState<AlertsTabKey>("alerts");
  const [filters, setFilters] = useState<AlertListQuery>({});
  const [selectedAlertId, setSelectedAlertId] = useState<string | null>(null);
  const [selectedAlertSnapshot, setSelectedAlertSnapshot] =
    useState<AlertRecord | null>(null);

  // Action loading states
  const [actingAlertId, setActingAlertId] = useState<string | null>(null);
  const [convertingAlertId, setConvertingAlertId] = useState<string | null>(
    null
  );
  const [createdWorkItem, setCreatedWorkItem] = useState<{
    code: string;
    alertId: string;
  } | null>(null);
  const [batchAcking, setBatchAcking] = useState(false);
  const [savingRule, setSavingRule] = useState(false);
  const [dryRunningId, setDryRunningId] = useState<string | null>(null);
  const [dryRunResult, setDryRunResult] = useState<RuleDryRunResponse | null>(
    null
  );
  const [savingChannel, setSavingChannel] = useState(false);
  const [testingChannelId, setTestingChannelId] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<
    Record<string, TestChannelResponse>
  >({});
  const [savingSilence, setSavingSilence] = useState(false);

  // Feedback banners
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [noticeMessage, setNoticeMessage] = useState<string | null>(null);

  const handleConvertAlertToWorkItem = useCallback(async (alert: AlertRecord) => {
    setErrorMessage(null);
    setNoticeMessage(null);
    setConvertingAlertId(alert.id);
    try {
      const created = await axopsApi.createWorkItem({
        title: `[告警处置] ${alert.title}`,
        description: alert.body || `告警去重键 ${alert.dedup_key} 转入交付追踪`,
        item_kind: "ISSUE",
        priority: alert.severity === "CRITICAL" ? "P0" : "P1",
        source_kind: "ALERT",
        source_ref: alert.id,
        node_ids: alert.node_id ? [alert.node_id] : [],
      });
      const code = created.code?.trim() || `WI-${created.item_no}`;
      setCreatedWorkItem({ code, alertId: alert.id });
    } catch (err) {
      setErrorMessage(formatErrorMessage(err, "转工作项失败"));
    } finally {
      setConvertingAlertId(null);
    }
  }, []);

  const hasActiveFilters = Boolean(
    filters.status ||
      filters.severity ||
      filters.rule_id ||
      filters.node_id ||
      filters.type_name
  );

  // 1. Operational Alerts Queue (`refetchInterval: 15_000` per Step 08 §08.5)
  const alertsQuery = useQuery({
    queryKey: ["axalerts", "alerts", filters],
    queryFn: () => listAlerts({ ...filters, limit: 100 }),
    refetchInterval: 15_000,
  });

  // Unfiltered summary query only runs when user filters the table so KPIs stay accurate
  const summaryAlertsQuery = useQuery({
    queryKey: ["axalerts", "alerts-summary"],
    queryFn: () => listAlerts({ limit: 200 }),
    enabled: hasActiveFilters,
    refetchInterval: 15_000,
  });

  // 2. Rules Registry
  const rulesQuery = useQuery({
    queryKey: ["axalerts", "rules"],
    queryFn: () => listRules({ limit: 100 }),
  });

  // 3. Notification Channels
  const channelsQuery = useQuery({
    queryKey: ["axalerts", "channels"],
    queryFn: () => listNotifyChannels({ limit: 100 }),
  });

  // 4. Alert Silences
  const silencesQuery = useQuery({
    queryKey: ["axalerts", "silences"],
    queryFn: () => listAlertSilences({ limit: 100 }),
  });

  // 5. Reference metadata for RuleEditor & filters (graceful fallback on empty/stub responses)
  const flowsQuery = useQuery({
    queryKey: ["axalerts", "flows"],
    queryFn: async (): Promise<FlowRecord[]> => {
      try {
        const res = await axflowApi.listFlows({ limit: 100 });
        return res.items;
      } catch {
        return [];
      }
    },
  });

  const datasetsQuery = useQuery({
    queryKey: ["axalerts", "datasets"],
    queryFn: async (): Promise<Dataset[]> => {
      try {
        const res = await axhubApi.listDatasets({ limit: 100 });
        return Array.isArray(res?.items) ? res.items : [];
      } catch {
        return [];
      }
    },
  });

  const schemaTypesQuery = useQuery({
    queryKey: ["axalerts", "schema-types"],
    queryFn: async (): Promise<string[]> => {
      try {
        const schema = await xblueApi.getSchema();
        if (!Array.isArray(schema?.node_types)) return [];
        return schema.node_types
          .map((nt) => (typeof nt?.name === "string" ? nt.name.trim() : ""))
          .filter((name) => name.length > 0);
      } catch {
        return [];
      }
    },
  });

  const alerts: AlertRecord[] = useMemo(
    () => alertsQuery.data?.items ?? [],
    [alertsQuery.data]
  );
  const kpiAlerts: AlertRecord[] = useMemo(
    () =>
      hasActiveFilters
        ? (summaryAlertsQuery.data?.items ?? alerts)
        : alerts,
    [alerts, hasActiveFilters, summaryAlertsQuery.data]
  );
  const rules: RuleDef[] = useMemo(
    () => rulesQuery.data?.items ?? [],
    [rulesQuery.data]
  );
  const channels: NotifyChannelRecord[] = useMemo(
    () => channelsQuery.data?.items ?? [],
    [channelsQuery.data]
  );
  const silences: AlertSilenceRecord[] = useMemo(
    () => silencesQuery.data?.items ?? [],
    [silencesQuery.data]
  );
  const flows: FlowRecord[] = useMemo(
    () => flowsQuery.data ?? [],
    [flowsQuery.data]
  );
  const datasets: Dataset[] = useMemo(
    () => datasetsQuery.data ?? [],
    [datasetsQuery.data]
  );

  const knownTypeNames = useMemo(() => {
    const names = new Set<string>();
    for (const t of schemaTypesQuery.data ?? []) {
      if (t) names.add(t);
    }
    for (const r of rules) {
      if (
        r.scope_kind === "METRIC" &&
        typeof r.target.type_name === "string" &&
        r.target.type_name.trim().length > 0
      ) {
        names.add(r.target.type_name.trim());
      }
    }
    for (const a of kpiAlerts) {
      if (
        typeof a.detail.type_name === "string" &&
        a.detail.type_name.trim().length > 0
      ) {
        names.add(a.detail.type_name.trim());
      }
    }
    if (names.size === 0) {
      names.add("Warehouse");
    }
    return Array.from(names);
  }, [kpiAlerts, rules, schemaTypesQuery.data]);

  const kpiMetrics = useMemo(() => {
    let openCount = 0;
    let criticalOpenCount = 0;
    let ackCount = 0;
    let resolvedCount = 0;

    for (const a of kpiAlerts) {
      if (a.status === "OPEN") {
        openCount += 1;
        if (a.severity === "CRITICAL") {
          criticalOpenCount += 1;
        }
      } else if (a.status === "ACK") {
        ackCount += 1;
      } else if (a.status === "RESOLVED") {
        resolvedCount += 1;
      }
    }

    const enabledRulesCount = rules.filter((r) => r.enabled).length;
    const enabledChannelsCount = channels.filter((c) => c.enabled).length;
    const activeSilencesCount = silences.filter(isSilenceActiveNow).length;

    return {
      openCount,
      criticalOpenCount,
      ackCount,
      resolvedCount,
      enabledRulesCount,
      totalRulesCount: rules.length,
      enabledChannelsCount,
      totalChannelsCount: channels.length,
      activeSilencesCount,
      totalSilencesCount: silences.length,
    };
  }, [channels, kpiAlerts, rules, silences]);

  const selectedAlert = useMemo(() => {
    if (!selectedAlertId) return null;
    const fromList = alerts.find((a) => a.id === selectedAlertId);
    if (fromList) return fromList;
    if (selectedAlertSnapshot && selectedAlertSnapshot.id === selectedAlertId) {
      return selectedAlertSnapshot;
    }
    return null;
  }, [alerts, selectedAlertId, selectedAlertSnapshot]);

  const selectedAlertRule = useMemo(() => {
    if (!selectedAlert) return null;
    return rules.find((r) => r.id === selectedAlert.rule_id) ?? null;
  }, [rules, selectedAlert]);

  const refreshAlertsCaches = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["axalerts", "alerts"] }),
      queryClient.invalidateQueries({
        queryKey: ["axalerts", "alerts-summary"],
      }),
      queryClient.invalidateQueries({ queryKey: ["axalerts", "alert-detail"] }),
      queryClient.invalidateQueries({ queryKey: ["axalerts", "alert-events"] }),
    ]);
  }, [queryClient]);

  const handleSelectAlert = useCallback(
    (alertId: string) => {
      setSelectedAlertId(alertId);
      const found = alerts.find((a) => a.id === alertId) ?? null;
      if (found) {
        setSelectedAlertSnapshot(found);
      }
    },
    [alerts]
  );

  const handleAckAlert = useCallback(
    async (alertId: string, comment = "") => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setActingAlertId(alertId);
      try {
        const updated = await ackAlert(alertId, { comment });
        setSelectedAlertSnapshot((prev) =>
          prev?.id === alertId || selectedAlertId === alertId ? updated : prev
        );
        await refreshAlertsCaches();
        setNoticeMessage(`告警已确认 (ACK)：${updated.title}`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "确认告警失败"));
      } finally {
        setActingAlertId(null);
      }
    },
    [refreshAlertsCaches, selectedAlertId]
  );

  const handleResolveAlert = useCallback(
    async (alertId: string, comment = "") => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setActingAlertId(alertId);
      try {
        const updated = await resolveAlert(alertId, { comment });
        setSelectedAlertSnapshot((prev) =>
          prev?.id === alertId || selectedAlertId === alertId ? updated : prev
        );
        await refreshAlertsCaches();
        setNoticeMessage(`告警已手动恢复 (RESOLVED)：${updated.title}`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "手动恢复告警失败"));
      } finally {
        setActingAlertId(null);
      }
    },
    [refreshAlertsCaches, selectedAlertId]
  );

  const handleBatchAck = useCallback(
    async (alertIds: string[], comment: string) => {
      if (alertIds.length === 0) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setBatchAcking(true);
      try {
        const res = await ackAlertsBatch({ alert_ids: alertIds, comment });
        await refreshAlertsCaches();
        setNoticeMessage(`批量确认完成：共确认 ${res.acked_count} 条活动告警`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "批量确认告警失败"));
      } finally {
        setBatchAcking(false);
      }
    },
    [refreshAlertsCaches]
  );

  const handleCreateRule = useCallback(
    async (req: CreateRuleRequest) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        const created = await createRule(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "rules"],
        });
        setNoticeMessage(`规则已创建：${created.name} (v${created.version})`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "创建规则失败"));
      } finally {
        setSavingRule(false);
      }
    },
    [queryClient]
  );

  const handleUpdateRule = useCallback(
    async (id: string, req: UpdateRuleRequest) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        const updated = await updateRule(id, req);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["axalerts", "rules"] }),
          refreshAlertsCaches(),
        ]);
        setNoticeMessage(`规则已更新：${updated.name} (v${updated.version})`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "更新规则失败"));
      } finally {
        setSavingRule(false);
      }
    },
    [queryClient, refreshAlertsCaches]
  );

  const handleDeleteRule = useCallback(
    async (id: string) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        await deleteRule(id);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["axalerts", "rules"] }),
          refreshAlertsCaches(),
        ]);
        setNoticeMessage("规则已删除，关联未恢复告警已自动归档");
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "删除规则失败"));
      } finally {
        setSavingRule(false);
      }
    },
    [queryClient, refreshAlertsCaches]
  );

  const handleDryRunSaved = useCallback(async (id: string) => {
    setErrorMessage(null);
    setNoticeMessage(null);
    setDryRunningId(id);
    try {
      const result = await dryRunSavedRule(id);
      setDryRunResult(result);
      setNoticeMessage(
        `规则试判定完成：扫描 ${result.evaluated_objects} 个对象，命中 ${result.matched_count} 条候选告警`
      );
    } catch (err) {
      setErrorMessage(formatErrorMessage(err, "规则试判定失败"));
    } finally {
      setDryRunningId(null);
    }
  }, []);

  const handleDryRunAdhoc = useCallback(
    async (req: CreateRuleRequest, editingRuleId: string | null) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setDryRunningId("adhoc");
      try {
        const result = editingRuleId
          ? await dryRunSavedRule(editingRuleId)
          : await dryRunAdhocRule(req);
        setDryRunResult(result);
        setNoticeMessage(
          `试判定预览完成：扫描 ${result.evaluated_objects} 个对象，命中 ${result.matched_count} 条候选告警`
        );
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "试判定预览失败"));
      } finally {
        setDryRunningId(null);
      }
    },
    []
  );

  const handleCreateChannel = useCallback(
    async (req: CreateNotifyChannelRequest) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        const created = await createNotifyChannel(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage(`通知渠道已创建：${created.name} (${created.kind})`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "创建通知渠道失败"));
      } finally {
        setSavingChannel(false);
      }
    },
    [queryClient]
  );

  const handleUpdateChannel = useCallback(
    async (id: string, req: UpdateNotifyChannelRequest) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        const updated = await updateNotifyChannel(id, req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage(`通知渠道已更新：${updated.name}`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "更新通知渠道失败"));
      } finally {
        setSavingChannel(false);
      }
    },
    [queryClient]
  );

  const handleDeleteChannel = useCallback(
    async (id: string) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        await deleteNotifyChannel(id);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage("通知渠道已删除");
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "删除通知渠道失败"));
      } finally {
        setSavingChannel(false);
      }
    },
    [queryClient]
  );

  const handleTestChannel = useCallback(async (id: string) => {
    setErrorMessage(null);
    setNoticeMessage(null);
    setTestingChannelId(id);
    try {
      const res = await testNotifyChannel(id);
      setTestResults((prev) => ({ ...prev, [id]: res }));
      if (res.ok && res.delivery.status === "SENT") {
        setNoticeMessage(
          `测试投递成功 (${res.delivery.status}${
            res.delivery.response_code !== null
              ? ` · HTTP ${res.delivery.response_code}`
              : ""
          })`
        );
      } else {
        setErrorMessage(
          res.delivery.error_message ??
            `测试投递未送达 (status=${res.delivery.status})`
        );
      }
    } catch (err) {
      setErrorMessage(formatErrorMessage(err, "测试通知渠道失败"));
    } finally {
      setTestingChannelId(null);
    }
  }, []);

  const handleCreateSilence = useCallback(
    async (req: CreateSilenceRequest) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingSilence(true);
      try {
        const created = await createAlertSilence(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "silences"],
        });
        setNoticeMessage(`静默窗口已创建：${created.reason}`);
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "创建静默窗口失败"));
      } finally {
        setSavingSilence(false);
      }
    },
    [queryClient]
  );

  const handleDeleteSilence = useCallback(
    async (id: string) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingSilence(true);
      try {
        await deleteAlertSilence(id);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "silences"],
        });
        setNoticeMessage("静默窗口已移除");
      } catch (err) {
        setErrorMessage(formatErrorMessage(err, "移除静默窗口失败"));
      } finally {
        setSavingSilence(false);
      }
    },
    [queryClient]
  );

  const tabs: readonly {
    key: AlertsTabKey;
    label: string;
    testId: string;
    icon: typeof Bell;
    count: number;
    highlight?: boolean;
  }[] = [
    {
      key: "alerts",
      label: "告警中心",
      testId: "tab-alerts",
      icon: Bell,
      count: kpiMetrics.openCount,
      highlight: kpiMetrics.openCount > 0,
    },
    {
      key: "rules",
      label: "规则引擎",
      testId: "tab-rules",
      icon: SlidersHorizontal,
      count: rules.length,
    },
    {
      key: "channels",
      label: "通知渠道",
      testId: "tab-channels",
      icon: Send,
      count: channels.length,
    },
    {
      key: "silences",
      label: "静默窗口",
      testId: "tab-silences",
      icon: BellOff,
      count: silences.length,
    },
  ];

  return (
    <div className="space-y-5" data-testid="axalerts-page">
      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxAlerts · 告警与规则中心
          </h1>
          <p className="text-xs text-muted-foreground">
            指标、数据质量与事件规则判定，告警去重、升级、静默与多渠道通知投递
          </p>
        </div>

        <div className="flex items-center gap-2">
          <Badge variant="xblue" data-testid="alerts-polling-badge">
            <Activity className="h-3 w-3 mr-1" />
            15s 自动轮询
          </Badge>
          <Badge variant="outline" className="font-mono-tech">
            Step 08 · AxFlow Rules
          </Badge>
        </div>
      </div>

      {/* KPI Summary Cards */}
      <div
        className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3"
        data-testid="axalerts-kpi-grid"
      >
        <Card className="p-3.5 space-y-1.5" data-testid="kpi-open-alerts">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>待确认活动告警 (OPEN)</span>
            <AlertTriangle
              className={cn(
                "h-3.5 w-3.5",
                kpiMetrics.openCount > 0
                  ? "text-red-600 dark:text-red-400"
                  : "text-muted-foreground"
              )}
            />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {kpiMetrics.openCount}
            </span>
            <Badge
              variant={kpiMetrics.criticalOpenCount > 0 ? "xblue" : "outline"}
              className="font-mono-tech"
            >
              CRITICAL ×{kpiMetrics.criticalOpenCount}
            </Badge>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-acked-alerts">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>处理中告警 (ACK)</span>
            <CheckCircle2 className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {kpiMetrics.ackCount}
            </span>
            <span className="text-[11px] font-mono-tech text-muted-foreground">
              已恢复 ×{kpiMetrics.resolvedCount}
            </span>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-active-rules">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>启用判定规则 (Rules)</span>
            <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {kpiMetrics.enabledRulesCount}
            </span>
            <span className="text-[11px] font-mono-tech text-muted-foreground">
              共 {kpiMetrics.totalRulesCount} 条定义
            </span>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-channels-silences">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>投递渠道 / 生效静默</span>
            <Send className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {kpiMetrics.enabledChannelsCount}
            </span>
            <Badge variant="outline" className="font-mono-tech">
              静默生效 ×{kpiMetrics.activeSilencesCount}
            </Badge>
          </div>
        </Card>
      </div>

      {/* Feedback Banners */}
      {errorMessage && (
        <div
          role="alert"
          data-testid="axalerts-error-banner"
          className="flex items-center justify-between gap-3 rounded-md border border-red-500/40 bg-red-500/10 px-3.5 py-2.5 text-xs text-red-700 dark:text-red-300"
        >
          <span>{errorMessage}</span>
          <button
            type="button"
            onClick={() => setErrorMessage(null)}
            className="text-[11px] underline cursor-pointer shrink-0"
          >
            关闭
          </button>
        </div>
      )}

      {noticeMessage && (
        <div
          data-testid="axalerts-notice-banner"
          className="flex items-center justify-between gap-3 rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2.5 text-xs text-xblue"
        >
          <span>{noticeMessage}</span>
          <button
            type="button"
            onClick={() => setNoticeMessage(null)}
            className="text-[11px] underline cursor-pointer shrink-0"
          >
            关闭
          </button>
        </div>
      )}

      {createdWorkItem && (
        <div
          data-testid="axalerts-workitem-banner"
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-xblue/40 bg-xblue-soft px-3.5 py-2.5 text-xs text-xblue"
        >
          <div className="flex flex-wrap items-center gap-2">
            <Kanban className="h-3.5 w-3.5" />
            <span className="font-medium">
              已创建工作项 {createdWorkItem.code}
            </span>
            <Link
              to="/ops"
              data-testid="axalerts-workitem-ops-link"
              className="inline-flex items-center gap-1 underline font-semibold"
            >
              前往 AxOps 查看交付进度 (/ops)
              <ExternalLink className="h-3 w-3" />
            </Link>
          </div>
          <button
            type="button"
            onClick={() => setCreatedWorkItem(null)}
            className="text-[11px] underline cursor-pointer shrink-0"
          >
            关闭
          </button>
        </div>
      )}

      {/* Workspace Tabs Navigation */}
      <div
        role="tablist"
        aria-label="AxAlerts 工作台视图"
        className="flex flex-wrap items-center gap-1.5 border-b border-border pb-2"
        data-testid="axalerts-tabs"
      >
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={isActive}
              data-testid={tab.testId}
              onClick={() => setActiveTab(tab.key)}
              className={cn(
                "inline-flex items-center gap-2 rounded-md px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer",
                isActive
                  ? "bg-foreground text-background"
                  : "text-muted-foreground hover:text-foreground hover:bg-muted"
              )}
            >
              <Icon
                className={cn(
                  "h-3.5 w-3.5",
                  !isActive && tab.highlight && "text-red-600 dark:text-red-400"
                )}
              />
              <span>{tab.label}</span>
              <span
                className={cn(
                  "inline-flex items-center justify-center rounded px-1.5 py-0.2 text-[10px] font-mono-tech",
                  isActive
                    ? "bg-background/20 text-background"
                    : tab.highlight
                      ? "bg-red-500/15 text-red-600 dark:text-red-400"
                      : "bg-muted text-muted-foreground"
                )}
              >
                {tab.count}
              </span>
            </button>
          );
        })}
      </div>

      {/* Tab Panels */}
      {activeTab === "alerts" && (
        <>
          <AlertTable
            alerts={alerts}
            rules={rules}
            knownTypeNames={knownTypeNames}
            filters={filters}
            selectedAlertId={selectedAlertId}
            loading={alertsQuery.isFetching}
            actingAlertId={actingAlertId}
            batchAcking={batchAcking}
            onFiltersChange={setFilters}
            onSelectAlert={handleSelectAlert}
            onAckAlert={(id) => handleAckAlert(id, "")}
            onResolveAlert={(id) => handleResolveAlert(id, "")}
            onBatchAck={handleBatchAck}
            onRefresh={() => {
              void refreshAlertsCaches();
            }}
          />

          {alerts.length > 0 && (
            <Card
              className="p-3.5 space-y-2.5"
              data-testid="axalerts-workitem-dispatch-card"
            >
              <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                <div className="flex items-center gap-2 font-medium">
                  <Kanban className="h-3.5 w-3.5 text-xblue" />
                  <span>告警协同转工作项（写入 AxOps 交付看板 · ISSUE）</span>
                </div>
                <span className="text-[11px] text-muted-foreground">
                  告警恢复时将自动在关联工作项上追加 LINK 事件
                </span>
              </div>

              <div className="divide-y divide-border/60">
                {alerts.slice(0, 10).map((alert) => {
                  const isConverting = convertingAlertId === alert.id;
                  return (
                    <div
                      key={alert.id}
                      data-testid={`alert-workitem-row-${alert.id}`}
                      className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs"
                    >
                      <div className="flex flex-wrap items-center gap-2 min-w-0">
                        <Badge
                          variant={
                            alert.severity === "CRITICAL" ? "xblue" : "outline"
                          }
                        >
                          {alert.severity}
                        </Badge>
                        <Badge variant="outline">{alert.status}</Badge>
                        <span className="font-medium truncate">
                          {alert.title}
                        </span>
                      </div>

                      <Button
                        type="button"
                        variant="outline"
                        className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                        data-testid={`convert-alert-workitem-btn-${alert.id}`}
                        disabled={isConverting}
                        onClick={() => void handleConvertAlertToWorkItem(alert)}
                      >
                        <Kanban className="h-3 w-3" />
                        {isConverting ? "创建中..." : "转工作项"}
                      </Button>
                    </div>
                  );
                })}
              </div>
            </Card>
          )}

          {selectedAlert && (
            <>
              <AlertDetail
                alert={selectedAlert}
                rule={selectedAlertRule}
                channels={channels}
                acting={actingAlertId === selectedAlert.id}
                onClose={() => setSelectedAlertId(null)}
                onAck={handleAckAlert}
                onResolve={handleResolveAlert}
              />
              <div className="fixed top-4 right-14 z-[60] flex items-center gap-2">
                {createdWorkItem &&
                  createdWorkItem.alertId === selectedAlert.id && (
                    <Link
                      to="/ops"
                      data-testid="alert-detail-workitem-link"
                      className="inline-flex items-center gap-1 rounded-md border border-xblue/40 bg-xblue-soft px-2.5 py-1.5 text-[11px] font-mono-tech font-semibold text-xblue shadow-sm"
                    >
                      已创建工作项 {createdWorkItem.code}
                      <ExternalLink className="h-3 w-3" />
                    </Link>
                  )}
                <Button
                  type="button"
                  variant="outline"
                  className="h-8 px-2.5 text-xs bg-background border-xblue/40 text-xblue hover:bg-xblue-soft shadow-sm"
                  data-testid="alert-detail-convert-workitem-btn"
                  disabled={convertingAlertId === selectedAlert.id}
                  onClick={() =>
                    void handleConvertAlertToWorkItem(selectedAlert)
                  }
                >
                  <Kanban className="h-3.5 w-3.5" />
                  {convertingAlertId === selectedAlert.id
                    ? "创建中..."
                    : "转工作项"}
                </Button>
              </div>
            </>
          )}
        </>
      )}

      {activeTab === "rules" && (
        <RuleEditor
          rules={rules}
          channels={channels}
          flows={flows}
          datasets={datasets}
          knownTypeNames={knownTypeNames}
          saving={savingRule}
          dryRunningId={dryRunningId}
          dryRunResult={dryRunResult}
          onCreateRule={handleCreateRule}
          onUpdateRule={handleUpdateRule}
          onDeleteRule={handleDeleteRule}
          onDryRunSaved={handleDryRunSaved}
          onDryRunAdhoc={handleDryRunAdhoc}
        />
      )}

      {activeTab === "channels" && (
        <ChannelForm
          channels={channels}
          saving={savingChannel}
          testingChannelId={testingChannelId}
          testResults={testResults}
          onCreateChannel={handleCreateChannel}
          onUpdateChannel={handleUpdateChannel}
          onDeleteChannel={handleDeleteChannel}
          onTestChannel={handleTestChannel}
        />
      )}

      {activeTab === "silences" && (
        <SilenceForm
          silences={silences}
          rules={rules}
          knownTypeNames={knownTypeNames}
          saving={savingSilence}
          onCreateSilence={handleCreateSilence}
          onDeleteSilence={handleDeleteSilence}
        />
      )}
    </div>
  );
}
