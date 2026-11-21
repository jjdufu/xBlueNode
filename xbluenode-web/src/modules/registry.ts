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

export type TabKey =
  | "dashboard"
  | "axhub"
  | "axanalytics"
  | "axingest"
  | "axquery"
  | "xbluegraph"
  | "xbluestudio"
  | "axflow"
  | "axalerts"
  | "axagent"
  | "agent_embed"
  | "axsim"
  | "xblueview"
  | "xblueview_share"
  | "axguard"
  | "axops";

export interface ModuleRouteSpec {
  key: TabKey;
  path: string;
  subPaths?: string[];
  label: string;
  icon: typeof LayoutDashboard;
  component: ComponentType;
  public?: boolean;
  wideLayout?: boolean;
  immersiveQueryParam?: string;
  requiredAnyPermissions?: readonly string[];
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  { key: "dashboard", path: "/", label: "系统概览", icon: LayoutDashboard, component: DashboardPage },
  { key: "axhub", path: "/hub", label: "AxHub · 数据接入与目录", icon: Cable, component: AxHubPage },
  { key: "axanalytics", path: "/analytics", label: "AxAnalytics · 分析与问数", icon: BarChart3, component: AnalyticsStudioPage },
  { key: "axingest", path: "/ingest", label: "AxIngest · 知识入库", icon: Database, component: AxIngestPage },
  { key: "axquery", path: "/query", label: "AxQuery · 智能问答", icon: MessageSquare, component: AxQueryPage },
  { key: "xbluegraph", path: "/graph", label: "xBlueGraph · 图浏览", icon: Share2, component: XBlueGraphPage },
  { key: "xbluestudio", path: "/studio", label: "xBlueStudio · 节点建模", icon: Blocks, component: XBlueStudioPage },
  { key: "axflow", path: "/flow", label: "AxFlow · 编排", icon: Workflow, component: AxFlowPage },
  { key: "axalerts", path: "/alerts", label: "AxAlerts · 告警中心", icon: Bell, component: AxAlertsPage },
  { key: "axagent", path: "/agents", label: "AxAgent · 智能体与发布", icon: Bot, component: AxAgentPage },
  { key: "axsim", path: "/sim", label: "AxSim · 因果推演", icon: GitBranch, component: AxSimPage },
  {
    key: "xblueview",
    path: "/view",
    subPaths: ["/view/:slug"],
    label: "xBlueView · 孪生驾驶舱",
    icon: Monitor,
    component: XBlueViewPage,
    wideLayout: true,
    immersiveQueryParam: "screen",
  },
  {
    key: "axguard",
    path: "/guard",
    label: "AxGuard · 安全治理",
    icon: ShieldCheck,
    component: AxGuardPage,
    wideLayout: true,
    requiredAnyPermissions: ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"],
  },
  {
    key: "axops",
    path: "/ops",
    label: "AxOps · 运维与交付",
    icon: ServerCog,
    component: AxOpsPage,
    wideLayout: true,
    requiredAnyPermissions: ["OPS_ADMIN", "DATA_READ", "FLOW_RUN"],
  },
  { key: "agent_embed", path: "/share/agent/:token", label: "AxAgent 嵌入对话", icon: Bot, component: AgentEmbedPage, public: true },
  { key: "xblueview_share", path: "/share/:token", label: "xBlueView 安全分享", icon: Monitor, component: SharePage, public: true },
];
