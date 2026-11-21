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
import { Badge, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

interface AxGuardTabSpec {
  readonly key: AxGuardTabKey;
  readonly label: string;
  readonly icon: typeof Users;
  readonly access: "admin" | "audit" | "grants";
}

const AXGUARD_TABS: readonly AxGuardTabSpec[] = [
  { key: "users", label: "用户管理", icon: Users, access: "admin" },
  { key: "roles", label: "角色与权限", icon: Shield, access: "admin" },
  { key: "depts", label: "部门树", icon: FolderTree, access: "admin" },
  { key: "policies", label: "ABAC 策略", icon: Code2, access: "admin" },
  { key: "row_policies", label: "行级策略", icon: Filter, access: "admin" },
  { key: "column_policies", label: "列脱敏", icon: EyeOff, access: "admin" },
  { key: "grants", label: "跨部门授权", icon: FileKey2, access: "grants" },
  { key: "audit", label: "审计日志", icon: ScrollText, access: "audit" },
];

export function AxGuardPage() {
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
    } else if (hasGrantsAccess) {
      const [gRes, dsRes] = await Promise.allSettled([
        axguardApi.listGrants(),
        axhubApi.listDatasets({ limit: 100 }),
      ]);
      if (gRes.status === "fulfilled") setGrants(gRes.value);
      if (dsRes.status === "fulfilled" && Array.isArray(dsRes.value?.items)) {
        setDatasets(dsRes.value.items);
      }
    }
  }, [hasGuardAdmin, hasGrantsAccess]);

  useEffect(() => {
    if (!meResolved) return;
    void loadCoreGovernanceData();
  }, [meResolved, loadCoreGovernanceData]);

  const canAccessTab = (spec: AxGuardTabSpec): boolean => {
    if (spec.access === "admin") return hasGuardAdmin;
    if (spec.access === "audit") return hasAuditRead;
    return hasGrantsAccess;
  };

  if (!hasGuardAdmin && !hasAuditRead && !hasGrantsAccess) {
    return (
      <div className="space-y-4">
        <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
          <ShieldCheck className="h-5 w-5 text-xblue" />
          AxGuard · 安全治理中心
        </h1>
        <Card className="p-8 text-center space-y-2">
          <Lock className="h-6 w-6 text-muted-foreground mx-auto" />
          <div className="text-sm font-semibold">权限不足</div>
          <p className="text-xs text-muted-foreground">
            需要 GUARD_ADMIN 或 AUDIT_READ 权限访问安全治理与审计验真控制台。
          </p>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-5" data-testid="axguard-page">
      {/* Page Header */}
      <header className="flex flex-wrap items-start justify-between gap-4 border-b border-border pb-4">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <h1 className="text-xl font-semibold tracking-tight flex items-center gap-2">
              <ShieldCheck className="h-5 w-5 text-xblue" />
              AxGuard · 安全治理中心
            </h1>
            <Badge variant="xblue">Cedar ABAC · RLS · SHA-256 Chain</Badge>
          </div>
          <p className="text-xs text-muted-foreground">
            统一管控租户身份认证、RBAC 角色矩阵、部门层级、Cedar 资源策略、行列级动态脱敏、跨部门数据授权与防篡改审计链。
          </p>
        </div>

        {me && me.username && (
          <div className="flex items-center gap-2 text-xs font-mono-tech">
            <Badge variant="outline">租户: {me.tenant_id}</Badge>
            <Badge variant="outline">
              主体: {me.username} ({me.dept_id})
            </Badge>
          </div>
        )}
      </header>

      {/* 8-Panel Tab Bar */}
      <div
        role="tablist"
        aria-label="AxGuard 安全治理模块"
        className="flex flex-wrap items-center gap-1 rounded-md border border-border bg-muted/40 p-1"
      >
        {AXGUARD_TABS.map((tab) => {
          const Icon = tab.icon;
          const allowed = canAccessTab(tab);
          const active = activeTab === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={active}
              disabled={!allowed}
              data-testid={`axguard-tab-${tab.key}`}
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

        {activeTab === "audit" && hasAuditRead && <AuditExplorer />}
      </section>
    </div>
  );
}
