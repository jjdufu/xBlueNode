import type { ComponentType } from "react";
import {
  LayoutDashboard,
  Database,
  MessageSquare,
  Share2,
  Blocks,
  Workflow,
  GitBranch,
} from "lucide-react";
import { DashboardPage } from "@/pages/DashboardPage";
import { AxIngestPage } from "@/pages/AxIngestPage";
import { AxQueryPage } from "@/pages/AxQueryPage";
import { XBlueGraphPage } from "@/pages/XBlueGraphPage";
import { XBlueStudioPage } from "@/pages/XBlueStudioPage";
import { AxFlowPage } from "@/pages/AxFlowPage";
import { AxSimPage } from "@/pages/AxSimPage";

export type TabKey =
  | "dashboard"
  | "axingest"
  | "axquery"
  | "xbluegraph"
  | "xbluestudio"
  | "axflow"
  | "axsim";

export interface ModuleRouteSpec {
  key: TabKey;
  label: string;
  icon: typeof LayoutDashboard;
  component: ComponentType;
}

export const MODULE_REGISTRY: ModuleRouteSpec[] = [
  { key: "dashboard", label: "系统概览", icon: LayoutDashboard, component: DashboardPage },
  { key: "axingest", label: "AxIngest · 知识入库", icon: Database, component: AxIngestPage },
  { key: "axquery", label: "AxQuery · 五路问答", icon: MessageSquare, component: AxQueryPage },
  { key: "xbluegraph", label: "xBlueGraph · 拓扑视图", icon: Share2, component: XBlueGraphPage },
  { key: "xbluestudio", label: "xBlueStudio · 节点建模", icon: Blocks, component: XBlueStudioPage },
  { key: "axflow", label: "AxFlow · 编排流水线", icon: Workflow, component: AxFlowPage },
  { key: "axsim", label: "AxSim · 因果推演", icon: GitBranch, component: AxSimPage },
];
