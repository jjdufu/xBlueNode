import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router";
import {
  Cpu,
  ScrollText,
  ServerCog,
  ShieldCheck,
  Users,
} from "lucide-react";
import { getCachedMe } from "@/lib/api";
import {
  fetchLlmUsage,
  getUserLlmPreferences,
  listLlmProviders,
} from "@/lib/api/axagent";
import { ProviderTable } from "@/components/axagent/ProviderTable";
import { UserLlmPreferencesBar } from "@/components/axagent/UserLlmPreferencesBar";
import { PageHeader } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { AxGuardPage } from "@/pages/AxGuardPage";
import { AxOpsPage } from "@/pages/AxOpsPage";

export type SettingsGroup =
  | "members"
  | "security"
  | "models"
  | "ops"
  | "audit";

const MEMBERS_TABS = new Set([
  "access",
  "users",
  "roles",
  "depts",
  "members",
]);

const SECURITY_TABS = new Set([
  "policies",
  "row_policies",
  "column_policies",
  "masking",
  "grants",
]);

const MODELS_TABS = new Set([
  "models",
  "providers",
  "channels",
  "llm",
]);

const OPS_TABS = new Set([
  "health",
  "jobs",
  "retention",
  "backups",
  "templates",
  "work_items",
  "settings",
]);

function resolveDefaultGroupFromPrincipal(): SettingsGroup {
  const cached = getCachedMe();
  if (!cached || !Array.isArray(cached.permissions)) {
    return "members";
  }
  const roles = Array.isArray(cached.roles) ? cached.roles : [];
  const perms = cached.permissions as readonly string[];
  if (roles.includes("ADMIN") || perms.includes("GUARD_ADMIN")) {
    return "members";
  }
  if (perms.includes("AUDIT_READ") && !perms.includes("OPS_ADMIN") && !perms.includes("FLOW_RUN")) {
    return "audit";
  }
  if (perms.includes("OPS_ADMIN") || perms.includes("FLOW_RUN")) {
    return "ops";
  }
  if (perms.includes("DATA_READ")) {
    return "security";
  }
  return "members";
}

function resolveSettingsGroup(
  rawSection: string | null,
  rawTab: string | null
): SettingsGroup {
  const normalizedTab = (rawTab ?? "").trim().toLowerCase();
  if (MEMBERS_TABS.has(normalizedTab)) return "members";
  if (SECURITY_TABS.has(normalizedTab)) return "security";
  if (MODELS_TABS.has(normalizedTab)) return "models";
  if (OPS_TABS.has(normalizedTab)) return "ops";
  if (normalizedTab === "audit") return "audit";

  const normalizedSection = (rawSection ?? "").trim().toLowerCase();
  if (normalizedSection === "members") return "members";
  if (normalizedSection === "security") return "security";
  if (normalizedSection === "models") return "models";
  if (normalizedSection === "ops") return "ops";
  if (normalizedSection === "audit") return "audit";

  return resolveDefaultGroupFromPrincipal();
}

function SettingsModelChannelsPanel() {
  const queryClient = useQueryClient();
  const [usageFrom, setUsageFrom] = useState("");
  const [usageTo, setUsageTo] = useState("");

  const providersQuery = useQuery({
    queryKey: ["axagent", "providers"],
    queryFn: async () => {
      try {
        return await listLlmProviders();
      } catch {
        return [];
      }
    },
  });

  const llmPreferencesQuery = useQuery({
    queryKey: ["axagent", "llm-preferences"],
    queryFn: async () => {
      try {
        return await getUserLlmPreferences();
      } catch {
        return [];
      }
    },
  });

  const usageQuery = useQuery({
    queryKey: ["axagent", "usage", usageFrom, usageTo],
    queryFn: async () => {
      try {
        return await fetchLlmUsage({
          ...(usageFrom ? { from: usageFrom } : {}),
          ...(usageTo ? { to: usageTo } : {}),
        });
      } catch {
        return [];
      }
    },
  });

  const refreshModels = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["axagent", "providers"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "llm-preferences"] }),
      queryClient.invalidateQueries({ queryKey: ["axagent", "usage"] }),
    ]);
  }, [queryClient]);

  return (
    <div className="space-y-5" data-testid="settings-models-panel">
      <UserLlmPreferencesBar
        providers={providersQuery.data ?? []}
        preferences={llmPreferencesQuery.data ?? []}
        onRefresh={refreshModels}
      />
      <ProviderTable
        providers={providersQuery.data ?? []}
        usage={usageQuery.data ?? []}
        usageFrom={usageFrom}
        usageTo={usageTo}
        onChangeUsageRange={(from, to) => {
          setUsageFrom(from);
          setUsageTo(to);
        }}
        onRefresh={refreshModels}
      />
    </div>
  );
}

export function GovernancePage() {
  const { tx } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const rawSection = searchParams.get("section");
  const rawTab = searchParams.get("tab");
  const group = resolveSettingsGroup(rawSection, rawTab);

  const handleGroupChange = (next: SettingsGroup) => {
    const updated = new URLSearchParams(searchParams);
    updated.set("section", next);
    if (next === "members") {
      updated.set("tab", "users");
    } else if (next === "security") {
      updated.set("tab", "policies");
    } else if (next === "audit") {
      updated.set("tab", "audit");
    } else {
      updated.delete("tab");
    }
    setSearchParams(updated, { replace: true });
  };

  const navItems: ReadonlyArray<{
    key: SettingsGroup;
    label: string;
    icon: typeof Users;
    testId: string;
  }> = [
    {
      key: "members",
      label: tx("成员与权限", "Members & Roles"),
      icon: Users,
      testId: "governance-tab-members",
    },
    {
      key: "security",
      label: tx("安全与脱敏", "Security & Masking"),
      icon: ShieldCheck,
      testId: "governance-tab-security",
    },
    {
      key: "models",
      label: tx("模型通道", "Model Channels"),
      icon: Cpu,
      testId: "governance-tab-models",
    },
    {
      key: "ops",
      label: tx("任务与备份", "Jobs & Backups"),
      icon: ServerCog,
      testId: "governance-tab-ops",
    },
    {
      key: "audit",
      label: tx("审计日志", "Audit Logs"),
      icon: ScrollText,
      testId: "governance-tab-audit",
    },
  ];

  return (
    <div className="space-y-4" data-testid="governance-page">
      <PageHeader
        title={tx(MODULE_META.governance.label, MODULE_META.governance.enLabel)}
        badges={
          <nav
            aria-label={tx("系统设置分区", "Settings sections")}
            className="inline-flex h-8 items-center gap-0.5 rounded-md border border-border bg-muted/50 p-0.5 overflow-x-auto max-w-full"
          >
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = group === item.key;
              return (
                <button
                  key={item.key}
                  type="button"
                  data-testid={item.testId}
                  onClick={() => handleGroupChange(item.key)}
                  className={cn(
                    "inline-flex h-full items-center gap-1.5 rounded-[5px] px-2.5 text-xs whitespace-nowrap shrink-0 transition-colors cursor-pointer",
                    active
                      ? "bg-xblue-soft text-xblue font-semibold shadow-2xs"
                      : "font-medium text-muted-foreground hover:text-foreground"
                  )}
                >
                  <Icon className="h-3.5 w-3.5 shrink-0" />
                  <span>{item.label}</span>
                </button>
              );
            })}
          </nav>
        }
      />

      <div className="min-w-0">
        {group === "models" ? (
          <SettingsModelChannelsPanel />
        ) : group === "ops" ? (
          <AxOpsPage embedded />
        ) : (
          <AxGuardPage embedded group={group} />
        )}
      </div>
    </div>
  );
}
