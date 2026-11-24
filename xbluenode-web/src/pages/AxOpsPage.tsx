import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import {
  Activity,
  Archive,
  Clock,
  Cpu,
  Kanban,
  Lock,
  Package,
  ServerCog,
} from "lucide-react";
import { api, fetchMe, getCachedMe, type MeResponse } from "@/lib/api";
import {
  axopsApi,
  type DeepHealthReport,
  type OutboxStats,
  type SystemInfoResponse,
} from "@/lib/api/axops";
import { BackupPanel } from "@/components/axops/BackupPanel";
import type { AxOpsTabKey } from "@/components/axops/constants";
import { HealthPanel } from "@/components/axops/HealthPanel";
import { JobsPanel } from "@/components/axops/JobsPanel";
import {
  canAccessAxOpsTab,
  hasOpsAdminAccess,
  resolveAxOpsInitialTab,
} from "@/components/axops/logic";
import { RetentionPanel } from "@/components/axops/RetentionPanel";
import { TemplateGallery } from "@/components/axops/TemplateGallery";
import { WorkItemBoard } from "@/components/axops/WorkItemBoard";
import { Card, PageHeader, Tabs } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";

interface AxOpsTabSpec {
  readonly key: AxOpsTabKey;
  readonly zhLabel: string;
  readonly enLabel: string;
  readonly icon: typeof Activity;
}

const AXOPS_TABS: readonly AxOpsTabSpec[] = [
  {
    key: "health",
    zhLabel: "深度健康巡检",
    enLabel: "Deep Health",
    icon: Activity,
  },
  {
    key: "jobs",
    zhLabel: "异步任务队列",
    enLabel: "Async Jobs",
    icon: Cpu,
  },
  {
    key: "retention",
    zhLabel: "数据保留策略",
    enLabel: "Retention",
    icon: Clock,
  },
  {
    key: "backups",
    zhLabel: "一致性备份",
    enLabel: "Backups",
    icon: Archive,
  },
  {
    key: "templates",
    zhLabel: "行业模板与演示",
    enLabel: "Templates & Demos",
    icon: Package,
  },
  {
    key: "work_items",
    zhLabel: "工作项与交付",
    enLabel: "Work Items",
    icon: Kanban,
  },
];

export function AxOpsPage({
  embedded = false,
}: {
  readonly embedded?: boolean;
} = {}) {
  const { isZh, tx } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const rawTabParam = searchParams.get("tab");
  const requestedTab = rawTabParam === "settings" ? "retention" : rawTabParam;
  const [me, setMe] = useState<MeResponse | null>(() => getCachedMe());
  const [meResolved, setMeResolved] = useState<boolean>(
    () => getCachedMe() !== null
  );
  const [activeTab, setActiveTab] = useState<AxOpsTabKey>(() => {
    const cached = getCachedMe();
    if (cached && Array.isArray(cached.permissions)) {
      return resolveAxOpsInitialTab(
        requestedTab,
        cached.permissions as readonly string[],
        cached.roles ?? []
      );
    }
    return resolveAxOpsInitialTab(
      requestedTab,
      ["OPS_ADMIN", "DATA_READ", "FLOW_RUN"],
      ["ADMIN"]
    );
  });

  const [healthReport, setHealthReport] = useState<DeepHealthReport | null>(
    null
  );
  const [outboxStats, setOutboxStats] = useState<OutboxStats | null>(null);
  const [systemInfo, setSystemInfo] = useState<SystemInfoResponse | null>(null);
  const [embeddingModel, setEmbeddingModel] = useState<string | null>(null);
  const [healthLoading, setHealthLoading] = useState(false);

  const effectivePerms = useMemo<readonly string[]>(() => {
    if (!me || !Array.isArray(me.permissions)) {
      return ["OPS_ADMIN", "DATA_READ", "FLOW_RUN"];
    }
    return me.permissions as readonly string[];
  }, [me]);

  const effectiveRoles = useMemo<readonly string[]>(() => {
    if (!me || !Array.isArray(me.roles)) {
      return ["ADMIN"];
    }
    return me.roles;
  }, [me]);

  const hasOpsAdmin = useMemo(
    () => hasOpsAdminAccess(effectivePerms, effectiveRoles),
    [effectivePerms, effectiveRoles]
  );

  const hasAnyOpsAccess = useMemo(
    () =>
      AXOPS_TABS.some((tab) =>
        canAccessAxOpsTab(tab.key, effectivePerms, effectiveRoles)
      ),
    [effectivePerms, effectiveRoles]
  );

  useEffect(() => {
    if (!requestedTab) return;
    const resolved = resolveAxOpsInitialTab(
      requestedTab,
      effectivePerms,
      effectiveRoles
    );
    if (resolved === requestedTab) {
      setActiveTab(resolved);
    }
  }, [effectivePerms, effectiveRoles, requestedTab]);

  useEffect(() => {
    let cancelled = false;
    void fetchMe()
      .then((profile) => {
        if (cancelled) return;
        if (profile && Array.isArray(profile.permissions)) {
          setMe(profile);
          setActiveTab((prev) => {
            const perms = profile.permissions as readonly string[];
            const roles = profile.roles ?? [];
            if (requestedTab) {
              const fromQuery = resolveAxOpsInitialTab(
                requestedTab,
                perms,
                roles
              );
              if (fromQuery === requestedTab) {
                return fromQuery;
              }
            }
            if (!canAccessAxOpsTab(prev, perms, roles)) {
              return resolveAxOpsInitialTab(null, perms, roles);
            }
            return prev;
          });
        }
        setMeResolved(true);
      })
      .catch(() => {
        if (!cancelled) {
          setMeResolved(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [requestedTab]);

  const loadHealthOverview = useCallback(async () => {
    if (!hasOpsAdmin) return;
    setHealthLoading(true);
    try {
      const [hRes, oRes, sRes, baseHealth] = await Promise.allSettled([
        axopsApi.getDeepHealth(),
        axopsApi.getOutboxStats(),
        axopsApi.getSystemInfo(),
        api.health(),
      ]);
      if (hRes.status === "fulfilled") setHealthReport(hRes.value);
      if (oRes.status === "fulfilled") setOutboxStats(oRes.value);
      if (sRes.status === "fulfilled") setSystemInfo(sRes.value);
      if (baseHealth.status === "fulfilled") {
        setEmbeddingModel(baseHealth.value?.embedding_model || null);
      }
    } finally {
      setHealthLoading(false);
    }
  }, [hasOpsAdmin]);

  useEffect(() => {
    if (!meResolved || !hasOpsAdmin) return;
    void loadHealthOverview();
  }, [hasOpsAdmin, loadHealthOverview, meResolved]);

  const tabItems = useMemo(
    () =>
      AXOPS_TABS.map((tab) => {
        const Icon = tab.icon;
        const allowed = canAccessAxOpsTab(
          tab.key,
          effectivePerms,
          effectiveRoles
        );
        return {
          key: tab.key,
          label: isZh ? tab.zhLabel : tab.enLabel,
          icon: allowed ? (
            <Icon className="h-3.5 w-3.5" />
          ) : (
            <Lock className="h-3 w-3" />
          ),
          disabled: !allowed,
          testId: `axops-tab-${tab.key}`,
        };
      }),
    [effectivePerms, effectiveRoles, isZh]
  );

  if (!hasAnyOpsAccess) {
    return (
      <div className="space-y-4">
        {!embedded && (
          <PageHeader
            title={
              <span className="inline-flex items-center gap-2">
                <ServerCog className="h-4 w-4 text-xblue" />
                {tx(MODULE_META.axops.label, MODULE_META.axops.enLabel)}
              </span>
            }
          />
        )}
        <Card className="p-8 text-center space-y-2">
          <Lock className="h-6 w-6 text-muted-foreground mx-auto" />
          <div className="text-sm font-semibold">
            {tx("权限不足", "Access Denied")}
          </div>
          <p className="text-xs text-muted-foreground">
            {tx(
              "当前账号未授权访问运维巡检与交付工作台。",
              "Current principal lacks permissions to access Operations & Delivery."
            )}
          </p>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-5" data-testid="axops-page">
      {!embedded && (
        <PageHeader
          title={
            <span className="inline-flex items-center gap-2">
              <ServerCog className="h-4 w-4 text-xblue" />
              {tx(MODULE_META.axops.label, MODULE_META.axops.enLabel)}
            </span>
          }
        />
      )}

      <Tabs
        items={tabItems}
        value={activeTab}
        onChange={(next) => {
          if (canAccessAxOpsTab(next, effectivePerms, effectiveRoles)) {
            setActiveTab(next);
            const updated = new URLSearchParams(searchParams);
            updated.set("tab", next);
            setSearchParams(updated, { replace: true });
          }
        }}
      />

      <section>
        {activeTab === "health" && hasOpsAdmin && (
          <HealthPanel
            report={healthReport}
            outboxStats={outboxStats}
            systemInfo={systemInfo}
            embeddingModel={embeddingModel}
            loading={healthLoading}
            onRefresh={loadHealthOverview}
          />
        )}

        {activeTab === "jobs" &&
          canAccessAxOpsTab("jobs", effectivePerms, effectiveRoles) && (
            <JobsPanel canManageOps={hasOpsAdmin} />
          )}

        {activeTab === "retention" && hasOpsAdmin && <RetentionPanel />}

        {activeTab === "backups" && hasOpsAdmin && (
          <BackupPanel tenantId={me?.tenant_id ?? "default"} />
        )}

        {activeTab === "templates" &&
          canAccessAxOpsTab("templates", effectivePerms, effectiveRoles) && (
            <TemplateGallery canManageOps={hasOpsAdmin} />
          )}

        {activeTab === "work_items" &&
          canAccessAxOpsTab("work_items", effectivePerms, effectiveRoles) && (
            <WorkItemBoard />
          )}
      </section>
    </div>
  );
}
