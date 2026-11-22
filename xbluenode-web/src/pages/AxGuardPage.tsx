import { useCallback, useEffect, useMemo, useState } from "react";
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
import { Badge, Card, PageHeader, Tabs } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

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

export function AxGuardPage() {
  const { isZh, tx } = useI18n();
  const [me, setMe] = useState<MeResponse | null>(() => getCachedMe());
  const [meResolved, setMeResolved] = useState<boolean>(
    () => getCachedMe() !== null
  );
  const [activeTab, setActiveTab] = useState<AxGuardTabKey>(() => {
    const cached = getCachedMe();
    if (cached && Array.isArray(cached.permissions)) {
      return resolveDefaultAxGuardTab(
        cached.permissions as readonly string[],
        cached.roles ?? []
      );
    }
    return "users";
  });

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
    let cancelled = false;
    void fetchMe()
      .then((profile) => {
        if (cancelled) return;
        if (profile && Array.isArray(profile.permissions)) {
          setMe(profile);
          setActiveTab((prev) => {
            const preferred = resolveDefaultAxGuardTab(
              profile.permissions as readonly string[],
              profile.roles ?? []
            );
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
  }, []);

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

  const tabItems = useMemo(
    () =>
      AXGUARD_TABS.map((tab) => {
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
    [canAccessTab, isZh]
  );

  if (!hasGuardAdmin && !hasAuditRead && !hasGrantsAccess) {
    return (
      <div className="space-y-4">
        <PageHeader
          title={
            <span className="inline-flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
              {tx("AxGuard · 安全治理中心", "AxGuard · Security Governance")}
            </span>
          }
        />
        <Card className="p-8 text-center space-y-2">
          <Lock className="h-6 w-6 text-muted-foreground mx-auto" />
          <div className="text-sm font-semibold">
            {tx("权限不足", "Access Denied")}
          </div>
          <p className="text-xs text-muted-foreground">
            {tx(
              "需要安全治理或审计查阅权限。",
              "Requires GUARD_ADMIN or AUDIT_READ permission."
            )}
          </p>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-5" data-testid="axguard-page">
      <PageHeader
        title={
          <span className="inline-flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
            {tx("AxGuard · 安全治理中心", "AxGuard · Security Governance")}
          </span>
        }
        badges={
          <Badge variant="xblue">Cedar ABAC · RLS · SHA-256 Chain</Badge>
        }
      />

      {/* 8-Panel Tab Bar */}
      <Tabs items={tabItems} value={activeTab} onChange={setActiveTab} />

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
