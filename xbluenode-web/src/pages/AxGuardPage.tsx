import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import {
  Code2,
  EyeOff,
  FileKey2,
  Filter,
  FolderTree,
  Lock,
  ScrollText,
  Shield,
  ShieldCheck,
  Users,
} from "lucide-react";
import { fetchMe, getCachedMe, requestJson, type MeResponse } from "@/lib/api";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import type { BlueNodeTypeDef } from "@/lib/api/xblue";
import {
  axguardApi,
  type CedarPolicyRecord,
  type ColumnPolicyRecord,
  type DepartmentRecord,
  type GrantRecord,
  type GuardRoleRecord,
  type GuardUserRecord,
  type RowPolicyDefaultRecord,
  type RowPolicyRecord,
} from "@/lib/api/axguard";
import { AuditExplorer } from "@/components/axguard/AuditExplorer";
import { ColumnPolicyEditor } from "@/components/axguard/ColumnPolicyEditor";
import { DeptTree } from "@/components/axguard/DeptTree";
import { GrantsPanel } from "@/components/axguard/GrantsPanel";
import {
  resolveDefaultAxGuardTab,
  type AxGuardTabKey,
} from "@/components/axguard/logic";
import { PolicyEditor } from "@/components/axguard/PolicyEditor";
import { RolesPanel } from "@/components/axguard/RolesPanel";
import { RowPolicyEditor } from "@/components/axguard/RowPolicyEditor";
import { UsersPanel } from "@/components/axguard/UsersPanel";
import { Card, PageHeader, Tabs } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";

interface AxGuardTabSpec {
  readonly key: AxGuardTabKey;
  readonly zhLabel: string;
  readonly enLabel: string;
  readonly icon: typeof Users;
  readonly access: "admin" | "audit" | "grants";
}

const AXGUARD_TABS: readonly AxGuardTabSpec[] = [
  {
    key: "users",
    zhLabel: "用户管理",
    enLabel: "Users",
    icon: Users,
    access: "admin",
  },
  {
    key: "roles",
    zhLabel: "角色与权限",
    enLabel: "Roles & Permissions",
    icon: Shield,
    access: "admin",
  },
  {
    key: "depts",
    zhLabel: "部门树",
    enLabel: "Departments",
    icon: FolderTree,
    access: "admin",
  },
  {
    key: "policies",
    zhLabel: "ABAC 策略",
    enLabel: "ABAC Policies",
    icon: Code2,
    access: "admin",
  },
  {
    key: "row_policies",
    zhLabel: "行级策略",
    enLabel: "Row Policies",
    icon: Filter,
    access: "admin",
  },
  {
    key: "column_policies",
    zhLabel: "列脱敏",
    enLabel: "Column Masking",
    icon: EyeOff,
    access: "admin",
  },
  {
    key: "grants",
    zhLabel: "跨部门授权",
    enLabel: "Cross-Dept Grants",
    icon: FileKey2,
    access: "grants",
  },
  {
    key: "audit",
    zhLabel: "审计日志",
    enLabel: "Audit Logs",
    icon: ScrollText,
    access: "audit",
  },
];

export type AxGuardEmbeddedGroup = "members" | "security" | "audit";

const GUARD_GROUP_TABS: Readonly<
  Record<AxGuardEmbeddedGroup, readonly AxGuardTabKey[]>
> = {
  members: ["users", "roles", "depts"],
  security: ["policies", "row_policies", "column_policies", "grants"],
  audit: ["audit"],
};

function mapRequestedGuardTab(raw: string | null): AxGuardTabKey | null {
  const val = (raw ?? "").trim().toLowerCase();
  if (val === "access" || val === "users") return "users";
  if (val === "roles") return "roles";
  if (val === "depts") return "depts";
  if (val === "policies") return "policies";
  if (val === "row_policies") return "row_policies";
  if (val === "column_policies" || val === "masking") return "column_policies";
  if (val === "grants") return "grants";
  if (val === "audit") return "audit";
  return null;
}

function resolveScopedGuardTab(
  group: AxGuardEmbeddedGroup | undefined,
  requestedTab: AxGuardTabKey | null,
  perms: readonly string[],
  roles: readonly string[]
): AxGuardTabKey {
  if (!group) {
    if (requestedTab) return requestedTab;
    return resolveDefaultAxGuardTab(perms, roles);
  }
  const allowedKeys = GUARD_GROUP_TABS[group];
  if (requestedTab && allowedKeys.includes(requestedTab)) {
    return requestedTab;
  }
  if (group === "audit") {
    return "audit";
  }
  if (group === "security") {
    const isAdmin = roles.includes("ADMIN") || perms.includes("GUARD_ADMIN");
    return isAdmin ? "policies" : "grants";
  }
  return "users";
}

export function AxGuardPage({
  embedded = false,
  group,
}: {
  readonly embedded?: boolean;
  readonly group?: AxGuardEmbeddedGroup;
} = {}) {
  const { isZh, tx } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const effectiveGroup = embedded ? group : undefined;
  const requestedTab = mapRequestedGuardTab(searchParams.get("tab"));
  const [me, setMe] = useState<MeResponse | null>(() => getCachedMe());
  const [meResolved, setMeResolved] = useState<boolean>(
    () => getCachedMe() !== null
  );
  const [activeTab, setActiveTab] = useState<AxGuardTabKey>(() => {
    const cached = getCachedMe();
    const perms =
      cached && Array.isArray(cached.permissions)
        ? (cached.permissions as readonly string[])
        : ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"];
    const roles =
      cached && Array.isArray(cached.roles) ? cached.roles : ["ADMIN"];
    return resolveScopedGuardTab(effectiveGroup, requestedTab, perms, roles);
  });

  const handleTabChange = (next: AxGuardTabKey) => {
    setActiveTab(next);
    const updated = new URLSearchParams(searchParams);
    updated.set("tab", next);
    setSearchParams(updated, { replace: true });
  };

  const [users, setUsers] = useState<GuardUserRecord[]>([]);
  const [roles, setRoles] = useState<GuardRoleRecord[]>([]);
  const [departments, setDepartments] = useState<DepartmentRecord[]>([]);
  const [policies, setPolicies] = useState<CedarPolicyRecord[]>([]);
  const [rowPolicies, setRowPolicies] = useState<RowPolicyRecord[]>([]);
  const [rowDefaults, setRowDefaults] = useState<RowPolicyDefaultRecord[]>([]);
  const [columnPolicies, setColumnPolicies] = useState<ColumnPolicyRecord[]>(
    []
  );
  const [grants, setGrants] = useState<GrantRecord[]>([]);
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [nodeTypes, setNodeTypes] = useState<BlueNodeTypeDef[]>([]);

  const effectivePerms = useMemo<readonly string[]>(() => {
    if (!me || !Array.isArray(me.permissions)) {
      return ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"];
    }
    return me.permissions as readonly string[];
  }, [me]);

  const effectiveRoles = useMemo<readonly string[]>(() => {
    if (!me || !Array.isArray(me.roles)) {
      return ["ADMIN"];
    }
    return me.roles;
  }, [me]);

  const hasGuardAdmin =
    effectiveRoles.includes("ADMIN") || effectivePerms.includes("GUARD_ADMIN");
  const hasAuditRead = hasGuardAdmin || effectivePerms.includes("AUDIT_READ");
  const hasGrantsAccess = hasGuardAdmin || effectivePerms.includes("DATA_READ");

  useEffect(() => {
    if (effectiveGroup) {
      const allowedKeys = GUARD_GROUP_TABS[effectiveGroup];
      if (requestedTab && allowedKeys.includes(requestedTab)) {
        setActiveTab(requestedTab);
      } else if (!allowedKeys.includes(activeTab)) {
        setActiveTab(
          resolveScopedGuardTab(
            effectiveGroup,
            requestedTab,
            effectivePerms,
            effectiveRoles
          )
        );
      }
      return;
    }
    if (requestedTab) {
      setActiveTab(requestedTab);
    }
  }, [activeTab, effectiveGroup, effectivePerms, effectiveRoles, requestedTab]);

  useEffect(() => {
    let cancelled = false;
    void fetchMe()
      .then((profile) => {
        if (cancelled) return;
        if (profile && Array.isArray(profile.permissions)) {
          setMe(profile);
          setActiveTab((prev) => {
            const perms = profile.permissions as readonly string[];
            const profRoles = profile.roles ?? [];
            if (effectiveGroup) {
              const allowedKeys = GUARD_GROUP_TABS[effectiveGroup];
              if (allowedKeys.includes(prev)) {
                return prev;
              }
              return resolveScopedGuardTab(
                effectiveGroup,
                requestedTab,
                perms,
                profRoles
              );
            }
            const preferred = resolveDefaultAxGuardTab(perms, profRoles);
            return prev === "users" ? preferred : prev;
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
  }, [effectiveGroup, requestedTab]);

  const loadCoreGovernanceData = useCallback(async () => {
    if (hasGuardAdmin) {
      const [
        uRes,
        rRes,
        dRes,
        pRes,
        rpRes,
        rdRes,
        cpRes,
        gRes,
        dsRes,
        schemaRes,
      ] = await Promise.allSettled([
        axguardApi.listUsers(),
        axguardApi.listRoles(),
        axguardApi.listDepartments(),
        axguardApi.listPolicies(),
        axguardApi.listRowPolicies(),
        axguardApi.listRowPolicyDefaults(),
        axguardApi.listColumnPolicies(),
        axguardApi.listGrants(),
        axhubApi.listDatasets({ limit: 100 }),
        requestJson<{ node_types?: BlueNodeTypeDef[] }>("/api/xblue/schema"),
      ]);

      if (uRes.status === "fulfilled") setUsers(uRes.value);
      if (rRes.status === "fulfilled") setRoles(rRes.value);
      if (dRes.status === "fulfilled") setDepartments(dRes.value);
      if (pRes.status === "fulfilled") setPolicies(pRes.value);
      if (rpRes.status === "fulfilled") setRowPolicies(rpRes.value);
      if (rdRes.status === "fulfilled") setRowDefaults(rdRes.value);
      if (cpRes.status === "fulfilled") setColumnPolicies(cpRes.value);
      if (gRes.status === "fulfilled") setGrants(gRes.value);
      if (dsRes.status === "fulfilled" && Array.isArray(dsRes.value?.items)) {
        setDatasets(dsRes.value.items);
      }
      if (
        schemaRes.status === "fulfilled" &&
        Array.isArray(schemaRes.value?.node_types)
      ) {
        setNodeTypes(schemaRes.value.node_types);
      }
    } else if (hasGrantsAccess || hasAuditRead) {
      const [gRes, dsRes, dRes] = await Promise.allSettled([
        hasGrantsAccess ? axguardApi.listGrants() : Promise.resolve([]),
        hasGrantsAccess
          ? axhubApi.listDatasets({ limit: 100 })
          : Promise.resolve({ items: [], next_cursor: null }),
        hasGrantsAccess ? axguardApi.listDepartments() : Promise.resolve([]),
      ]);
      if (gRes.status === "fulfilled") setGrants(gRes.value);
      if (dsRes.status === "fulfilled" && Array.isArray(dsRes.value?.items)) {
        setDatasets(dsRes.value.items);
      }
      if (dRes.status === "fulfilled") setDepartments(dRes.value);
    }
  }, [hasAuditRead, hasGuardAdmin, hasGrantsAccess]);

  useEffect(() => {
    if (!meResolved) return;
    void loadCoreGovernanceData();
  }, [meResolved, loadCoreGovernanceData]);

  const canAccessTab = useCallback(
    (spec: AxGuardTabSpec): boolean => {
      if (spec.access === "admin") return hasGuardAdmin;
      if (spec.access === "audit") return hasAuditRead;
      return hasGrantsAccess;
    },
    [hasAuditRead, hasGrantsAccess, hasGuardAdmin]
  );

  const visibleSpecs = useMemo(() => {
    if (!effectiveGroup) return AXGUARD_TABS;
    const allowedKeys = GUARD_GROUP_TABS[effectiveGroup];
    return AXGUARD_TABS.filter((tab) => allowedKeys.includes(tab.key));
  }, [effectiveGroup]);

  const tabItems = useMemo(
    () =>
      visibleSpecs.map((tab) => {
        const Icon = tab.icon;
        const allowed = canAccessTab(tab);
        return {
          key: tab.key,
          label: (
            <span className="inline-flex items-center gap-1">
              <span>{isZh ? tab.zhLabel : tab.enLabel}</span>
              {!allowed && <Lock className="h-2.5 w-2.5" />}
            </span>
          ),
          icon: <Icon className="h-3.5 w-3.5" />,
          disabled: !allowed,
          testId: `axguard-tab-${tab.key}`,
        };
      }),
    [canAccessTab, isZh, visibleSpecs]
  );

  if (!hasGuardAdmin && !hasAuditRead && !hasGrantsAccess) {
    return (
      <div className="space-y-4">
        {!embedded && (
          <PageHeader
            title={
              <span className="inline-flex items-center gap-2">
                <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
                {tx(MODULE_META.axguard.label, MODULE_META.axguard.enLabel)}
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
              "需要安全治理或审计查阅权限。",
              "Requires security admin or audit read permission."
            )}
          </p>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-5" data-testid="axguard-page">
      {!embedded && (
        <PageHeader
          title={
            <span className="inline-flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
              {tx(MODULE_META.axguard.label, MODULE_META.axguard.enLabel)}
            </span>
          }
        />
      )}

      {effectiveGroup !== "audit" && (
        <Tabs items={tabItems} value={activeTab} onChange={handleTabChange} />
      )}

      {/* Active Sub-Panel */}
      <section>
        {activeTab === "users" && hasGuardAdmin && (
          <UsersPanel
            users={users}
            roles={roles}
            departments={departments}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "roles" && hasGuardAdmin && (
          <RolesPanel roles={roles} onReload={loadCoreGovernanceData} />
        )}

        {activeTab === "depts" && hasGuardAdmin && (
          <DeptTree
            departments={departments}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "policies" && hasGuardAdmin && (
          <PolicyEditor
            policies={policies}
            users={users}
            departments={departments}
            datasets={datasets}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "row_policies" && hasGuardAdmin && (
          <RowPolicyEditor
            rowPolicies={rowPolicies}
            rowDefaults={rowDefaults}
            datasets={datasets}
            nodeTypes={nodeTypes}
            users={users}
            roles={roles}
            departments={departments}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "column_policies" && hasGuardAdmin && (
          <ColumnPolicyEditor
            columnPolicies={columnPolicies}
            datasets={datasets}
            nodeTypes={nodeTypes}
            roles={roles}
            departments={departments}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "grants" && hasGrantsAccess && (
          <GrantsPanel
            grants={grants}
            datasets={datasets}
            departments={departments}
            roles={roles}
            users={users}
            onReload={loadCoreGovernanceData}
          />
        )}

        {activeTab === "audit" && hasAuditRead && (
          <AuditExplorer datasets={datasets} users={users} />
        )}
      </section>
    </div>
  );
}
