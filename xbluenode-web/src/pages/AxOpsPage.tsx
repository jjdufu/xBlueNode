import { useCallback, useEffect, useMemo, useState } from "react";
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
import { fetchMe, getCachedMe, type MeResponse } from "@/lib/api";
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
  resolveDefaultAxOpsTab,
} from "@/components/axops/logic";
import { RetentionPanel } from "@/components/axops/RetentionPanel";
import { TemplateGallery } from "@/components/axops/TemplateGallery";
import { WorkItemBoard } from "@/components/axops/WorkItemBoard";
import { Badge, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

interface AxOpsTabSpec {
  readonly key: AxOpsTabKey;
  readonly label: string;
  readonly icon: typeof Activity;
}

const AXOPS_TABS: readonly AxOpsTabSpec[] = [
  { key: "health", label: "深度健康巡检", icon: Activity },
  { key: "jobs", label: "异步任务队列", icon: Cpu },
  { key: "retention", label: "数据保留策略", icon: Clock },
  { key: "backups", label: "一致性备份", icon: Archive },
  { key: "templates", label: "行业模板与演示", icon: Package },
  { key: "work_items", label: "工作项与交付", icon: Kanban },
];

export function AxOpsPage() {
  const [me, setMe] = useState<MeResponse | null>(() => getCachedMe());
  const [meResolved, setMeResolved] = useState<boolean>(
    () => getCachedMe() !== null
  );
  const [activeTab, setActiveTab] = useState<AxOpsTabKey>(() => {
    const cached = getCachedMe();
    if (cached && Array.isArray(cached.permissions)) {
      return resolveDefaultAxOpsTab(
        cached.permissions as readonly string[],
        cached.roles ?? []
      );
    }
    return "health";
  });

  const [healthReport, setHealthReport] = useState<DeepHealthReport | null>(
    null
  );
  const [outboxStats, setOutboxStats] = useState<OutboxStats | null>(null);
  const [systemInfo, setSystemInfo] = useState<SystemInfoResponse | null>(null);
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
    let cancelled = false;
    void fetchMe()
      .then((profile) => {
        if (cancelled) return;
        if (profile && Array.isArray(profile.permissions)) {
          setMe(profile);
          setActiveTab((prev) => {
            const perms = profile.permissions as readonly string[];
            const roles = profile.roles ?? [];
            if (!canAccessAxOpsTab(prev, perms, roles)) {
              return resolveDefaultAxOpsTab(perms, roles);
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
  }, []);

  const loadHealthOverview = useCallback(async () => {
    if (!hasOpsAdmin) return;
    setHealthLoading(true);
    try {
      const [hRes, oRes, sRes] = await Promise.allSettled([
        axopsApi.getDeepHealth(),
        axopsApi.getOutboxStats(),
        axopsApi.getSystemInfo(),
      ]);
      if (hRes.status === "fulfilled") setHealthReport(hRes.value);
      if (oRes.status === "fulfilled") setOutboxStats(oRes.value);
      if (sRes.status === "fulfilled") setSystemInfo(sRes.value);
    } finally {
      setHealthLoading(false);
    }
  }, [hasOpsAdmin]);

  useEffect(() => {
    if (!meResolved || !hasOpsAdmin) return;
    void loadHealthOverview();
  }, [hasOpsAdmin, loadHealthOverview, meResolved]);

  if (!hasAnyOpsAccess) {
    return (
      <div className="space-y-4">
        <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
          <ServerCog className="h-5 w-5 text-xblue" />
          AxOps · 运维与交付中心
        </h1>
        <Card className="p-8 text-center space-y-2">
          <Lock className="h-6 w-6 text-muted-foreground mx-auto" />
          <div className="text-sm font-semibold">权限不足</div>
          <p className="text-xs text-muted-foreground">
            需要 OPS_ADMIN、DATA_READ 或 FLOW_RUN 权限访问运维巡检与交付工作台。
          </p>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-5" data-testid="axops-page">
      {/* Page Header */}
      <header className="flex flex-wrap items-start justify-between gap-4 border-b border-border pb-4">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
              <ServerCog className="h-5 w-5 text-xblue" />
              AxOps · 运维与交付中心
            </h1>
            <Badge variant="xblue">HA · Backup · Templates · Delivery</Badge>
          </div>
          <p className="text-xs text-muted-foreground">
            统一管控全链路深度健康探针、异步任务队列、11 类数据保留策略、一致性备份演练、行业模板包与工作项闭环交付。
          </p>
        </div>

        {me && me.username && (
          <div className="flex items-center gap-2 text-xs font-mono-tech">
            <Badge variant="outline">租户: {me.tenant_id}</Badge>
            <Badge variant="outline">
              主体: {me.username} ({hasOpsAdmin ? "OPS_ADMIN" : "OPERATOR"})
            </Badge>
          </div>
        )}
      </header>

      {/* 6-Panel Tab Bar */}
      <div
        role="tablist"
        aria-label="AxOps 运维与交付模块"
        className="flex flex-wrap items-center gap-1 rounded-md border border-border bg-muted/40 p-1"
      >
        {AXOPS_TABS.map((tab) => {
          const Icon = tab.icon;
          const allowed = canAccessAxOpsTab(
            tab.key,
            effectivePerms,
            effectiveRoles
          );
          const active = activeTab === tab.key;

          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={active}
              disabled={!allowed}
              data-testid={`axops-tab-${tab.key}`}
              onClick={() => {
                if (allowed) {
                  setActiveTab(tab.key);
                }
              }}
              className={cn(
                "inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer disabled:opacity-40 disabled:pointer-events-none",
                active
                  ? "bg-background text-foreground border border-border shadow-xs"
                  : "text-muted-foreground hover:text-foreground hover:bg-background/50"
              )}
            >
              <Icon className={cn("h-3.5 w-3.5", active && "text-xblue")} />
              {tab.label}
              {!allowed && <Lock className="h-2.5 w-2.5 ml-0.5" />}
            </button>
          );
        })}
      </div>

      {/* Active Sub-Panel */}
      <section>
        {activeTab === "health" && hasOpsAdmin && (
          <HealthPanel
            report={healthReport}
            outboxStats={outboxStats}
            systemInfo={systemInfo}
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
