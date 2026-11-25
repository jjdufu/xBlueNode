import type { ComponentType } from "react";
import {
  LayoutDashboard,
  Cable,
  BarChart3,
  Blocks,
  Workflow,
  Bot,
  GitBranch,
  Monitor,
  Settings,
  ShieldCheck,
  ServerCog,
} from "lucide-react";
import {
  MODULE_META,
  type NavGroupKey,
  type TabKey,
} from "@/modules/meta";
import { DashboardPage } from "@/pages/DashboardPage";
import { AxHubPage } from "@/pages/AxHubPage";
import { AnalyticsStudioPage } from "@/pages/AnalyticsStudioPage";
import { XBlueStudioPage } from "@/pages/XBlueStudioPage";
import { AxFlowPage } from "@/pages/AxFlowPage";
import { AxAgentPage } from "@/pages/AxAgentPage";
import { AgentEmbedPage } from "@/pages/AgentEmbedPage";
import { AxSimPage } from "@/pages/AxSimPage";
import { XBlueViewPage } from "@/pages/XBlueViewPage";
import { SharePage } from "@/pages/SharePage";
import { GovernancePage } from "@/pages/GovernancePage";
import { AxGuardPage } from "@/pages/AxGuardPage";
import { AxOpsPage } from "@/pages/AxOpsPage";

export type { NavGroupKey, TabKey } from "@/modules/meta";
export { MODULE_META, NAV_GROUPS, useModuleTitle } from "@/modules/meta";

export interface LegacyRedirectSpec {
  readonly from: string;
  readonly toPathname: string;
  readonly defaultQuery: Readonly<Record<string, string>>;
}

/**
 * Legacy URL compatibility map preserving existing bookmarks, deep links, and gate URLs (Step 23 & Step 31).
 */
export const LEGACY_ROUTE_REDIRECTS: readonly LegacyRedirectSpec[] = [
  { from: "/ingest", toPathname: "/hub", defaultQuery: { tab: "knowledge" } },
  { from: "/query", toPathname: "/analytics", defaultQuery: { tab: "ask" } },
  { from: "/graph", toPathname: "/studio", defaultQuery: { tab: "explorer" } },
  { from: "/alerts", toPathname: "/flow", defaultQuery: { tab: "alerts" } },
  { from: "/governance", toPathname: "/settings", defaultQuery: {} },
];

export interface ModuleRouteSpec {
  key: TabKey;
  path: string;
  subPaths?: string[];
  activeMatchPrefixes?: readonly string[];
  shortZh: string;
  shortEn: string;
  label: string;
  enLabel: string;
  group?: NavGroupKey;
  keywords?: readonly string[];
  icon: typeof LayoutDashboard;
  component: ComponentType;
  public?: boolean;
  sidebarHidden?: boolean;
  wideLayout?: boolean;
  fullBleed?: boolean;
  autoCollapseSidebar?: boolean;
  immersiveQueryParam?: string;
  requiredAnyPermissions?: readonly string[];
  shortcutKey?: string;
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  {
    ...MODULE_META.dashboard,
    path: "/",
    icon: LayoutDashboard,
    component: DashboardPage,
    wideLayout: true,
    shortcutKey: "G O",
  },
  {
    ...MODULE_META.axhub,
    path: "/hub",
    icon: Cable,
    component: AxHubPage,
    wideLayout: true,
    shortcutKey: "G D",
  },
  {
    ...MODULE_META.xbluestudio,
    path: "/studio",
    icon: Blocks,
    component: XBlueStudioPage,
    wideLayout: true,
    fullBleed: true,
    shortcutKey: "G S",
  },
  {
    ...MODULE_META.axanalytics,
    path: "/analytics",
    icon: BarChart3,
    component: AnalyticsStudioPage,
    wideLayout: true,
    fullBleed: true,
    shortcutKey: "G E",
  },
  {
    ...MODULE_META.axsim,
    path: "/sim",
    icon: GitBranch,
    component: AxSimPage,
    wideLayout: true,
    shortcutKey: "G M",
  },
  {
    ...MODULE_META.axflow,
    path: "/flow",
    icon: Workflow,
    component: AxFlowPage,
    wideLayout: true,
    fullBleed: true,
    autoCollapseSidebar: true,
    shortcutKey: "G F",
  },
  {
    ...MODULE_META.xblueview,
    path: "/view",
    subPaths: ["/view/:slug"],
    icon: Monitor,
    component: XBlueViewPage,
    wideLayout: true,
    immersiveQueryParam: "screen",
    shortcutKey: "G V",
  },
  {
    ...MODULE_META.axagent,
    path: "/agents",
    icon: Bot,
    component: AxAgentPage,
    wideLayout: true,
    shortcutKey: "G A",
  },
  {
    ...MODULE_META.governance,
    path: "/settings",
    activeMatchPrefixes: ["/governance", "/guard", "/ops"],
    icon: Settings,
    component: GovernancePage,
    wideLayout: true,
    requiredAnyPermissions: [
      "GUARD_ADMIN",
      "AUDIT_READ",
      "OPS_ADMIN",
      "FLOW_RUN",
      "DATA_READ",
    ],
    shortcutKey: "G G",
  },
  {
    ...MODULE_META.axguard,
    path: "/guard",
    icon: ShieldCheck,
    component: AxGuardPage,
    sidebarHidden: true,
    wideLayout: true,
    requiredAnyPermissions: ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"],
  },
  {
    ...MODULE_META.axops,
    path: "/ops",
    icon: ServerCog,
    component: AxOpsPage,
    sidebarHidden: true,
    wideLayout: true,
    requiredAnyPermissions: ["OPS_ADMIN", "FLOW_RUN"],
  },
  {
    ...MODULE_META.agent_embed,
    path: "/share/agent/:token",
    icon: Bot,
    component: AgentEmbedPage,
    public: true,
  },
  {
    ...MODULE_META.xblueview_share,
    path: "/share/:token",
    icon: Monitor,
    component: SharePage,
    public: true,
  },
];
