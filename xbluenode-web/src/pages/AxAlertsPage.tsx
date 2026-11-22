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
import { Badge, Card, PageHeader, Tabs } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
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
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();

  const canAck = can("ALERT_ACK");
  const canEditRules = can("RULE_EDIT");
  const canSilence = canAck || canEditRules;

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

  const handleConvertAlertToWorkItem = useCallback(
    async (alert: AlertRecord) => {
      if (!canAck) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setConvertingAlertId(alert.id);
      try {
        const created = await axopsApi.createWorkItem({
          title: `[${tx("告警", "Alert")}] ${alert.title}`,
          description: alert.body || alert.dedup_key,
          item_kind: "ISSUE",
          priority: alert.severity === "CRITICAL" ? "P0" : "P1",
          source_kind: "ALERT",
          source_ref: alert.id,
          node_ids: alert.node_id ? [alert.node_id] : [],
        });
        const code = created.code?.trim() || `WI-${created.item_no}`;
        setCreatedWorkItem({ code, alertId: alert.id });
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("转工作项失败", "Failed to create work item"))
        );
      } finally {
        setConvertingAlertId(null);
      }
    },
    [canAck, tx]
  );

  const hasActiveFilters = Boolean(
    filters.status ||
      filters.severity ||
      filters.rule_id ||
      filters.node_id ||
      filters.type_name
  );

  // 1. Operational Alerts Queue (`refetchInterval: 15_000`)
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

  // 5. Reference metadata for RuleEditor & filters
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
      if (!canAck) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setActingAlertId(alertId);
      try {
        const updated = await ackAlert(alertId, { comment });
        setSelectedAlertSnapshot((prev) =>
          prev?.id === alertId || selectedAlertId === alertId ? updated : prev
        );
        await refreshAlertsCaches();
        setNoticeMessage(
          tx(`告警已确认：${updated.title}`, `Alert acknowledged: ${updated.title}`)
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("确认告警失败", "Failed to acknowledge alert"))
        );
      } finally {
        setActingAlertId(null);
      }
    },
    [canAck, refreshAlertsCaches, selectedAlertId, tx]
  );

  const handleResolveAlert = useCallback(
    async (alertId: string, comment = "") => {
      if (!canAck) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setActingAlertId(alertId);
      try {
        const updated = await resolveAlert(alertId, { comment });
        setSelectedAlertSnapshot((prev) =>
          prev?.id === alertId || selectedAlertId === alertId ? updated : prev
        );
        await refreshAlertsCaches();
        setNoticeMessage(
          tx(`告警已恢复：${updated.title}`, `Alert resolved: ${updated.title}`)
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("恢复告警失败", "Failed to resolve alert"))
        );
      } finally {
        setActingAlertId(null);
      }
    },
    [canAck, refreshAlertsCaches, selectedAlertId, tx]
  );

  const handleBatchAck = useCallback(
    async (alertIds: string[], comment: string) => {
      if (!canAck || alertIds.length === 0) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setBatchAcking(true);
      try {
        const res = await ackAlertsBatch({ alert_ids: alertIds, comment });
        await refreshAlertsCaches();
        setNoticeMessage(
          tx(
            `已确认 ${res.acked_count} 条活动告警`,
            `Acknowledged ${res.acked_count} alerts`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("批量确认失败", "Batch acknowledge failed"))
        );
      } finally {
        setBatchAcking(false);
      }
    },
    [canAck, refreshAlertsCaches, tx]
  );

  const handleCreateRule = useCallback(
    async (req: CreateRuleRequest) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        const created = await createRule(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "rules"],
        });
        setNoticeMessage(
          tx(
            `规则已创建：${created.name} (v${created.version})`,
            `Rule created: ${created.name} (v${created.version})`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("创建规则失败", "Failed to create rule"))
        );
      } finally {
        setSavingRule(false);
      }
    },
    [canEditRules, queryClient, tx]
  );

  const handleUpdateRule = useCallback(
    async (id: string, req: UpdateRuleRequest) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        const updated = await updateRule(id, req);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["axalerts", "rules"] }),
          refreshAlertsCaches(),
        ]);
        setNoticeMessage(
          tx(
            `规则已更新：${updated.name} (v${updated.version})`,
            `Rule updated: ${updated.name} (v${updated.version})`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("更新规则失败", "Failed to update rule"))
        );
      } finally {
        setSavingRule(false);
      }
    },
    [canEditRules, queryClient, refreshAlertsCaches, tx]
  );

  const handleDeleteRule = useCallback(
    async (id: string) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingRule(true);
      try {
        await deleteRule(id);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["axalerts", "rules"] }),
          refreshAlertsCaches(),
        ]);
        setNoticeMessage(tx("规则已删除", "Rule deleted"));
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("删除规则失败", "Failed to delete rule"))
        );
      } finally {
        setSavingRule(false);
      }
    },
    [canEditRules, queryClient, refreshAlertsCaches, tx]
  );

  const handleDryRunSaved = useCallback(
    async (id: string) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setDryRunningId(id);
      try {
        const result = await dryRunSavedRule(id);
        setDryRunResult(result);
        setNoticeMessage(
          tx(
            `试判定完成：扫描 ${result.evaluated_objects} 个对象，命中 ${result.matched_count} 条`,
            `Dry run complete: ${result.evaluated_objects} evaluated, ${result.matched_count} matched`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("试判定失败", "Dry run failed"))
        );
      } finally {
        setDryRunningId(null);
      }
    },
    [tx]
  );

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
          tx(
            `试判定完成：扫描 ${result.evaluated_objects} 个对象，命中 ${result.matched_count} 条`,
            `Dry run complete: ${result.evaluated_objects} evaluated, ${result.matched_count} matched`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("试判定失败", "Dry run failed"))
        );
      } finally {
        setDryRunningId(null);
      }
    },
    [tx]
  );

  const handleCreateChannel = useCallback(
    async (req: CreateNotifyChannelRequest) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        const created = await createNotifyChannel(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage(
          tx(
            `通知渠道已创建：${created.name}`,
            `Channel created: ${created.name}`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("创建渠道失败", "Failed to create channel"))
        );
      } finally {
        setSavingChannel(false);
      }
    },
    [canEditRules, queryClient, tx]
  );

  const handleUpdateChannel = useCallback(
    async (id: string, req: UpdateNotifyChannelRequest) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        const updated = await updateNotifyChannel(id, req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage(
          tx(
            `通知渠道已更新：${updated.name}`,
            `Channel updated: ${updated.name}`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("更新渠道失败", "Failed to update channel"))
        );
      } finally {
        setSavingChannel(false);
      }
    },
    [canEditRules, queryClient, tx]
  );

  const handleDeleteChannel = useCallback(
    async (id: string) => {
      if (!canEditRules) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingChannel(true);
      try {
        await deleteNotifyChannel(id);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "channels"],
        });
        setNoticeMessage(tx("通知渠道已删除", "Channel deleted"));
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("删除渠道失败", "Failed to delete channel"))
        );
      } finally {
        setSavingChannel(false);
      }
    },
    [canEditRules, queryClient, tx]
  );

  const handleTestChannel = useCallback(
    async (id: string) => {
      setErrorMessage(null);
      setNoticeMessage(null);
      setTestingChannelId(id);
      try {
        const res = await testNotifyChannel(id);
        setTestResults((prev) => ({ ...prev, [id]: res }));
        if (res.ok && res.delivery.status === "SENT") {
          setNoticeMessage(
            tx(
              `测试投递已送达 (${res.delivery.status})`,
              `Test delivery sent (${res.delivery.status})`
            )
          );
        } else {
          setErrorMessage(
            res.delivery.error_message ??
              tx(
                `测试投递未送达 (${res.delivery.status})`,
                `Test delivery failed (${res.delivery.status})`
              )
          );
        }
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("测试投递失败", "Test delivery failed"))
        );
      } finally {
        setTestingChannelId(null);
      }
    },
    [tx]
  );

  const handleCreateSilence = useCallback(
    async (req: CreateSilenceRequest) => {
      if (!canSilence) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingSilence(true);
      try {
        const created = await createAlertSilence(req);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "silences"],
        });
        setNoticeMessage(
          tx(
            `静默窗口已创建：${created.reason}`,
            `Silence created: ${created.reason}`
          )
        );
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("创建静默窗口失败", "Failed to create silence"))
        );
      } finally {
        setSavingSilence(false);
      }
    },
    [canSilence, queryClient, tx]
  );

  const handleDeleteSilence = useCallback(
    async (id: string) => {
      if (!canSilence) return;
      setErrorMessage(null);
      setNoticeMessage(null);
      setSavingSilence(true);
      try {
        await deleteAlertSilence(id);
        await queryClient.invalidateQueries({
          queryKey: ["axalerts", "silences"],
        });
        setNoticeMessage(tx("静默窗口已移除", "Silence removed"));
      } catch (err) {
        setErrorMessage(
          formatErrorMessage(err, tx("移除静默窗口失败", "Failed to remove silence"))
        );
      } finally {
        setSavingSilence(false);
      }
    },
    [canSilence, queryClient, tx]
  );

  const tabItems = useMemo(
    () => [
      {
        key: "alerts" as const,
        label: tx("告警队列", "Alerts"),
        testId: "tab-alerts",
        icon: (
          <Bell
            className={cn(
              "h-3.5 w-3.5",
              kpiMetrics.openCount > 0 && "text-red-600 dark:text-red-400"
            )}
          />
        ),
        count: kpiMetrics.openCount,
      },
      {
        key: "rules" as const,
        label: tx("判定规则", "Rules"),
        testId: "tab-rules",
        icon: <SlidersHorizontal className="h-3.5 w-3.5" />,
        count: rules.length,
      },
      {
        key: "channels" as const,
        label: tx("通知渠道", "Channels"),
        testId: "tab-channels",
        icon: <Send className="h-3.5 w-3.5" />,
        count: channels.length,
      },
      {
        key: "silences" as const,
        label: tx("静默窗口", "Silences"),
        testId: "tab-silences",
        icon: <BellOff className="h-3.5 w-3.5" />,
        count: silences.length,
      },
    ],
    [channels.length, kpiMetrics.openCount, rules.length, silences.length, tx]
  );

  return (
    <div className="space-y-5" data-testid="axalerts-page">
      <PageHeader
        title={tx("AxAlerts · 告警与规则中心", "AxAlerts · Alerts & Rules")}
        badges={
          <Badge variant="xblue" data-testid="alerts-polling-badge">
            <Activity className="h-3 w-3 mr-1" />
            {tx("15s 自动轮询", "15s Auto-Poll")}
          </Badge>
        }
      />

      {/* KPI Summary Cards */}
      <div
        className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3"
        data-testid="axalerts-kpi-grid"
      >
        <Card className="p-3.5 space-y-1.5" data-testid="kpi-open-alerts">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{tx("待确认告警", "Open Alerts")}</span>
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
              {fmtNum(kpiMetrics.openCount, 0)}
            </span>
            <Badge
              variant={kpiMetrics.criticalOpenCount > 0 ? "danger" : "outline"}
            >
              {tx("紧急", "Critical")} ×{fmtNum(kpiMetrics.criticalOpenCount, 0)}
            </Badge>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-acked-alerts">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{tx("处理中告警", "Acknowledged")}</span>
            <CheckCircle2 className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {fmtNum(kpiMetrics.ackCount, 0)}
            </span>
            <span className="text-[11px] font-mono-tech text-muted-foreground">
              {tx("已恢复", "Resolved")} ×{fmtNum(kpiMetrics.resolvedCount, 0)}
            </span>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-active-rules">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{tx("启用规则", "Active Rules")}</span>
            <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {fmtNum(kpiMetrics.enabledRulesCount, 0)}
            </span>
            <span className="text-[11px] font-mono-tech text-muted-foreground">
              / {fmtNum(kpiMetrics.totalRulesCount, 0)}
            </span>
          </div>
        </Card>

        <Card className="p-3.5 space-y-1.5" data-testid="kpi-channels-silences">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{tx("通知渠道 / 静默", "Channels / Silences")}</span>
            <Send className="h-3.5 w-3.5 text-xblue" />
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold font-mono-tech text-foreground">
              {fmtNum(kpiMetrics.enabledChannelsCount, 0)}
            </span>
            <Badge variant="outline">
              {tx("静默中", "Silenced")} ×{fmtNum(kpiMetrics.activeSilencesCount, 0)}
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
            {tx("关闭", "Close")}
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
            {tx("关闭", "Close")}
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
              {tx(
                `已创建工作项 ${createdWorkItem.code}`,
                `Work item created: ${createdWorkItem.code}`
              )}
            </span>
            <Link
              to="/ops"
              data-testid="axalerts-workitem-ops-link"
              className="inline-flex items-center gap-1 underline font-semibold"
            >
              {tx("查看工作项看板", "Open Work Items")}
              <ExternalLink className="h-3 w-3" />
            </Link>
          </div>
          <button
            type="button"
            onClick={() => setCreatedWorkItem(null)}
            className="text-[11px] underline cursor-pointer shrink-0"
          >
            {tx("关闭", "Close")}
          </button>
        </div>
      )}

      {/* Workspace Tabs Navigation */}
      <div data-testid="axalerts-tabs">
        <Tabs items={tabItems} value={activeTab} onChange={setActiveTab} />
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
            convertingAlertId={convertingAlertId}
            batchAcking={batchAcking}
            canAck={canAck}
            onFiltersChange={setFilters}
            onSelectAlert={handleSelectAlert}
            onAckAlert={(id) => handleAckAlert(id, "")}
            onResolveAlert={(id) => handleResolveAlert(id, "")}
            onConvertWorkItem={handleConvertAlertToWorkItem}
            onBatchAck={handleBatchAck}
            onRefresh={() => {
              void refreshAlertsCaches();
            }}
          />

          {selectedAlert && (
            <AlertDetail
              alert={selectedAlert}
              rule={selectedAlertRule}
              channels={channels}
              acting={actingAlertId === selectedAlert.id}
              canAck={canAck}
              convertingWorkItem={convertingAlertId === selectedAlert.id}
              createdWorkItemCode={
                createdWorkItem?.alertId === selectedAlert.id
                  ? createdWorkItem.code
                  : null
              }
              onClose={() => setSelectedAlertId(null)}
              onAck={handleAckAlert}
              onResolve={handleResolveAlert}
              onConvertWorkItem={handleConvertAlertToWorkItem}
            />
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
          canEdit={canEditRules}
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
          canEdit={canEditRules}
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
          canSilence={canSilence}
          onCreateSilence={handleCreateSilence}
          onDeleteSilence={handleDeleteSilence}
        />
      )}
    </div>
  );
}
