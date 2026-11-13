import type { ComponentType } from "react";
import {
  LayoutDashboard,
  Cable,
  Database,
  MessageSquare,
  Share2,
  Blocks,
  Workflow,
  GitBranch,
} from "lucide-react";
import { DashboardPage } from "@/pages/DashboardPage";
import { AxHubPage } from "@/pages/AxHubPage";
import { AxIngestPage } from "@/pages/AxIngestPage";
import { AxQueryPage } from "@/pages/AxQueryPage";
import { XBlueGraphPage } from "@/pages/XBlueGraphPage";
import { XBlueStudioPage } from "@/pages/XBlueStudioPage";
import { AxFlowPage } from "@/pages/AxFlowPage";
import { AxSimPage } from "@/pages/AxSimPage";

export type TabKey =
  | "dashboard"
  | "axhub"
  | "axingest"
  | "axquery"
  | "xbluegraph"
  | "xbluestudio"
  | "axflow"
  | "axsim";

export interface ModuleRouteSpec {
  key: TabKey;
  path: string;
  label: string;
  icon: typeof LayoutDashboard;
  component: ComponentType;
  public?: boolean;
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  { key: "dashboard", path: "/", label: "系统概览", icon: LayoutDashboard, component: DashboardPage },
  { key: "axhub", path: "/hub", label: "AxHub · 数据接入与目录", icon: Cable, component: AxHubPage },
  { key: "axingest", path: "/ingest", label: "AxIngest · 知识入库", icon: Database, component: AxIngestPage },
  { key: "axquery", path: "/query", label: "AxQuery · 智能问答", icon: MessageSquare, component: AxQueryPage },
  { key: "xbluegraph", path: "/graph", label: "xBlueGraph · 图浏览", icon: Share2, component: XBlueGraphPage },
  { key: "xbluestudio", path: "/studio", label: "xBlueStudio · 节点建模", icon: Blocks, component: XBlueStudioPage },
  { key: "axflow", path: "/flow", label: "AxFlow · 编排", icon: Workflow, component: AxFlowPage },
  { key: "axsim", path: "/sim", label: "AxSim · 因果推演", icon: GitBranch, component: AxSimPage },
];
