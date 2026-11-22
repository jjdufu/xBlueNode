import type { ComponentType } from "react";
import {
  LayoutDashboard,
  Cable,
  BarChart3,
  Database,
  MessageSquare,
  Share2,
  Blocks,
  Workflow,
  Bell,
  Bot,
  GitBranch,
  Monitor,
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
import { AxIngestPage } from "@/pages/AxIngestPage";
import { AxQueryPage } from "@/pages/AxQueryPage";
import { XBlueGraphPage } from "@/pages/XBlueGraphPage";
import { XBlueStudioPage } from "@/pages/XBlueStudioPage";
import { AxFlowPage } from "@/pages/AxFlowPage";
import { AxAlertsPage } from "@/pages/AxAlertsPage";
import { AxAgentPage } from "@/pages/AxAgentPage";
import { AgentEmbedPage } from "@/pages/AgentEmbedPage";
import { AxSimPage } from "@/pages/AxSimPage";
import { XBlueViewPage } from "@/pages/XBlueViewPage";
import { SharePage } from "@/pages/SharePage";
import { AxGuardPage } from "@/pages/AxGuardPage";
import { AxOpsPage } from "@/pages/AxOpsPage";

export type { NavGroupKey, TabKey } from "@/modules/meta";
export { MODULE_META, NAV_GROUPS, useModuleTitle } from "@/modules/meta";

export interface ModuleRouteSpec {
  key: TabKey;
  path: string;
  subPaths?: string[];
  productTag?: string;
  shortZh: string;
  shortEn: string;
  label: string;
  enLabel: string;
  group?: NavGroupKey;
  icon: typeof LayoutDashboard;
  component: ComponentType;
  public?: boolean;
  wideLayout?: boolean;
  fullBleed?: boolean;
  autoCollapseSidebar?: boolean;
  immersiveQueryParam?: string;
  requiredAnyPermissions?: readonly string[];
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  {
    ...MODULE_META.dashboard,
    path: "/",
    icon: LayoutDashboard,
    component: DashboardPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axhub,
    path: "/hub",
    icon: Cable,
    component: AxHubPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axingest,
    path: "/ingest",
    icon: Database,
    component: AxIngestPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axanalytics,
    path: "/analytics",
    icon: BarChart3,
    component: AnalyticsStudioPage,
    wideLayout: true,
    fullBleed: true,
  },
  {
    ...MODULE_META.axquery,
    path: "/query",
    icon: MessageSquare,
    component: AxQueryPage,
    wideLayout: true,
    fullBleed: true,
  },
  {
    ...MODULE_META.xblueview,
    path: "/view",
    subPaths: ["/view/:slug"],
    icon: Monitor,
    component: XBlueViewPage,
    wideLayout: true,
    immersiveQueryParam: "screen",
  },
  {
    ...MODULE_META.xbluegraph,
    path: "/graph",
    icon: Share2,
    component: XBlueGraphPage,
    wideLayout: true,
    fullBleed: true,
  },
  {
    ...MODULE_META.xbluestudio,
    path: "/studio",
    icon: Blocks,
    component: XBlueStudioPage,
    wideLayout: true,
    fullBleed: true,
  },
  {
    ...MODULE_META.axflow,
    path: "/flow",
    icon: Workflow,
    component: AxFlowPage,
    wideLayout: true,
    fullBleed: true,
    autoCollapseSidebar: true,
  },
  {
    ...MODULE_META.axagent,
    path: "/agents",
    icon: Bot,
    component: AxAgentPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axsim,
    path: "/sim",
    icon: GitBranch,
    component: AxSimPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axalerts,
    path: "/alerts",
    icon: Bell,
    component: AxAlertsPage,
    wideLayout: true,
  },
  {
    ...MODULE_META.axguard,
    path: "/guard",
    icon: ShieldCheck,
    component: AxGuardPage,
    wideLayout: true,
    requiredAnyPermissions: ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"],
  },
  {
    ...MODULE_META.axops,
    path: "/ops",
    icon: ServerCog,
    component: AxOpsPage,
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
