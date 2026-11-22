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
  enLabel: string;
  icon: typeof LayoutDashboard;
  component: ComponentType;
  public?: boolean;
  wideLayout?: boolean;
  immersiveQueryParam?: string;
  requiredAnyPermissions?: readonly string[];
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  {
    key: "dashboard",
    path: "/",
    label: "系统概览",
    enLabel: "Overview",
    icon: LayoutDashboard,
    component: DashboardPage,
    wideLayout: true,
  },
  {
    key: "axhub",
    path: "/hub",
    label: "AxHub · 数据接入与目录",
    enLabel: "AxHub · Data Fabric",
    icon: Cable,
    component: AxHubPage,
    wideLayout: true,
  },
  {
    key: "axanalytics",
    path: "/analytics",
    label: "AxAnalytics · 分析与问数",
    enLabel: "AxAnalytics · Smart BI",
    icon: BarChart3,
    component: AnalyticsStudioPage,
    wideLayout: true,
  },
  {
    key: "axingest",
    path: "/ingest",
    label: "AxIngest · 知识入库",
    enLabel: "AxIngest · Ingestion",
    icon: Database,
    component: AxIngestPage,
    wideLayout: true,
  },
  {
    key: "axquery",
    path: "/query",
    label: "AxQuery · 智能问答",
    enLabel: "AxQuery · Hybrid Q&A",
    icon: MessageSquare,
    component: AxQueryPage,
    wideLayout: true,
  },
  {
    key: "xbluegraph",
    path: "/graph",
    label: "xBlueGraph · 图浏览",
    enLabel: "xBlueGraph · Explorer",
    icon: Share2,
    component: XBlueGraphPage,
    wideLayout: true,
  },
  {
    key: "xbluestudio",
    path: "/studio",
    label: "xBlueStudio · 节点建模",
    enLabel: "xBlueStudio · Ontology",
    icon: Blocks,
    component: XBlueStudioPage,
    wideLayout: true,
  },
  {
    key: "axflow",
    path: "/flow",
    label: "AxFlow · 编排",
    enLabel: "AxFlow · Pipelines",
    icon: Workflow,
    component: AxFlowPage,
    wideLayout: true,
  },
  {
    key: "axalerts",
    path: "/alerts",
    label: "AxAlerts · 告警中心",
    enLabel: "AxAlerts · Monitoring",
    icon: Bell,
    component: AxAlertsPage,
    wideLayout: true,
  },
  {
    key: "axagent",
    path: "/agents",
    label: "AxAgent · 智能体与发布",
    enLabel: "AxAgent · Agents & API",
    icon: Bot,
    component: AxAgentPage,
    wideLayout: true,
  },
  {
    key: "axsim",
    path: "/sim",
    label: "AxSim · 因果推演",
    enLabel: "AxSim · Simulation",
    icon: GitBranch,
    component: AxSimPage,
    wideLayout: true,
  },
  {
    key: "xblueview",
    path: "/view",
    subPaths: ["/view/:slug"],
    label: "xBlueView · 孪生驾驶舱",
    enLabel: "xBlueView · Twin Cockpit",
    icon: Monitor,
    component: XBlueViewPage,
    wideLayout: true,
    immersiveQueryParam: "screen",
  },
  {
    key: "axguard",
    path: "/guard",
    label: "AxGuard · 安全治理",
    enLabel: "AxGuard · Governance",
    icon: ShieldCheck,
    component: AxGuardPage,
    wideLayout: true,
    requiredAnyPermissions: ["GUARD_ADMIN", "AUDIT_READ", "DATA_READ"],
  },
  {
    key: "axops",
    path: "/ops",
    label: "AxOps · 运维与交付",
    enLabel: "AxOps · Operations",
    icon: ServerCog,
    component: AxOpsPage,
    wideLayout: true,
    requiredAnyPermissions: ["OPS_ADMIN", "FLOW_RUN"],
  },
  {
    key: "agent_embed",
    path: "/share/agent/:token",
    label: "AxAgent 嵌入对话",
    enLabel: "AxAgent Embed",
    icon: Bot,
    component: AgentEmbedPage,
    public: true,
  },
  {
    key: "xblueview_share",
    path: "/share/:token",
    label: "xBlueView 安全分享",
    enLabel: "xBlueView Share",
    icon: Monitor,
    component: SharePage,
    public: true,
  },
];
