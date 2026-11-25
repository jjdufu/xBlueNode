import { useI18n } from "@/i18n";

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
  | "governance"
  | "axguard"
  | "axops";

export type NavGroupKey = "overview" | "workspace" | "governance";

export interface NavGroupMeta {
  readonly key: NavGroupKey;
  readonly zh: string;
  readonly en: string;
}

export const NAV_GROUPS: readonly NavGroupMeta[] = [
  { key: "overview", zh: "工作台", en: "Workspace" },
  { key: "workspace", zh: "核心业务域", en: "Core Domains" },
  { key: "governance", zh: "系统设置", en: "Settings" },
];

export interface ModuleTitleMeta {
  readonly key: TabKey;
  readonly shortZh: string;
  readonly shortEn: string;
  readonly label: string;
  readonly enLabel: string;
  readonly group?: NavGroupKey;
  readonly keywords?: readonly string[];
}

/**
 * Single Source of Truth (G1) for sidebar navigation labels and page H1 titles.
 * Enforces brand uniqueness, bilingual purity, and plain engineering modeling terminology (Step 23 & Step 31).
 */
export const MODULE_META: Readonly<Record<TabKey, ModuleTitleMeta>> = {
  dashboard: {
    key: "dashboard",
    shortZh: "总控看板",
    shortEn: "Dashboard",
    label: "总控看板",
    enLabel: "Dashboard",
    group: "overview",
    keywords: [
      "dashboard",
      "overview",
      "home",
      "pulse",
      "onboarding",
      "template",
      "总控看板",
      "概览",
      "态势",
      "向导",
      "模板",
    ],
  },
  axhub: {
    key: "axhub",
    shortZh: "数据与知识",
    shortEn: "Data Hub",
    label: "数据与知识",
    enLabel: "Data Hub",
    group: "workspace",
    keywords: [
      "connector",
      "dataset",
      "document",
      "knowledge",
      "lineage",
      "upload",
      "连接源",
      "数据集",
      "知识文档",
      "血缘",
    ],
  },
  axanalytics: {
    key: "axanalytics",
    shortZh: "查询与制图",
    shortEn: "Analytics & Charts",
    label: "查询与制图",
    enLabel: "Analytics & Charts",
    group: "workspace",
    keywords: [
      "ask",
      "sql",
      "pivot",
      "chart",
      "metric",
      "axql",
      "智能问答",
      "查询",
      "透视",
      "制图",
      "指标",
    ],
  },
  xbluestudio: {
    key: "xbluestudio",
    shortZh: "建模工作室",
    shortEn: "Modeling Studio",
    label: "建模工作室",
    enLabel: "Modeling Studio",
    group: "workspace",
    keywords: [
      "modeling",
      "model",
      "ontology",
      "schema",
      "node",
      "edge",
      "graph",
      "explorer",
      "action",
      "业务模型",
      "建模工作室",
      "本体",
      "建模",
      "图谱",
      "动作",
    ],
  },
  axflow: {
    key: "axflow",
    shortZh: "编排与告警",
    shortEn: "Automation & Alerts",
    label: "编排与告警",
    enLabel: "Automation & Alerts",
    group: "workspace",
    keywords: ["flow", "dag", "pipeline", "rule", "alert", "approval", "编排", "告警", "审批"],
  },
  axagent: {
    key: "axagent",
    shortZh: "智能体与接口",
    shortEn: "Agents & APIs",
    label: "智能体与接口",
    enLabel: "Agents & APIs",
    group: "workspace",
    keywords: ["agent", "mcp", "endpoint", "llm", "im", "bot", "智能体", "接口", "模型"],
  },
  axsim: {
    key: "axsim",
    shortZh: "推演沙盘",
    shortEn: "Simulation",
    label: "推演沙盘",
    enLabel: "Simulation",
    group: "workspace",
    keywords: ["simulation", "sandbox", "scenario", "what-if", "causal", "推演", "沙盘", "干预"],
  },
  xblueview: {
    key: "xblueview",
    shortZh: "数字看板",
    shortEn: "Cockpits",
    label: "数字看板",
    enLabel: "Cockpits",
    group: "workspace",
    keywords: ["dashboard", "cockpit", "export", "3d", "twin", "数字看板", "看板", "驾驶舱", "出品"],
  },
  governance: {
    key: "governance",
    shortZh: "系统设置",
    shortEn: "Settings",
    label: "系统设置",
    enLabel: "Settings",
    group: "governance",
    keywords: [
      "settings",
      "governance",
      "security",
      "rbac",
      "abac",
      "masking",
      "audit",
      "backup",
      "retention",
      "系统设置",
      "设置",
      "系统治理",
      "权限",
      "脱敏",
      "审计",
      "备份",
      "运维",
    ],
  },
  axingest: {
    key: "axingest",
    shortZh: "知识文档",
    shortEn: "Knowledge Docs",
    label: "知识文档",
    enLabel: "Knowledge Docs",
  },
  axquery: {
    key: "axquery",
    shortZh: "智能问答",
    shortEn: "Intelligent Q&A",
    label: "智能问答",
    enLabel: "Intelligent Q&A",
  },
  xbluegraph: {
    key: "xbluegraph",
    shortZh: "图谱浏览",
    shortEn: "Graph Explorer",
    label: "图谱浏览",
    enLabel: "Graph Explorer",
  },
  axalerts: {
    key: "axalerts",
    shortZh: "规则告警",
    shortEn: "Rules & Alerts",
    label: "规则告警",
    enLabel: "Rules & Alerts",
  },
  axguard: {
    key: "axguard",
    shortZh: "权限与安全",
    shortEn: "Access & Security",
    label: "权限与安全",
    enLabel: "Access & Security",
  },
  axops: {
    key: "axops",
    shortZh: "运维与备份",
    shortEn: "Operations & Backup",
    label: "运维与备份",
    enLabel: "Operations & Backup",
  },
  agent_embed: {
    key: "agent_embed",
    shortZh: "智能对话",
    shortEn: "Assistant",
    label: "智能对话",
    enLabel: "Assistant",
  },
  xblueview_share: {
    key: "xblueview_share",
    shortZh: "安全分享",
    shortEn: "Secure Share",
    label: "安全分享",
    enLabel: "Secure Share",
  },
};

export function getModuleTitle(key: TabKey, isZh: boolean): string {
  const meta = MODULE_META[key];
  return isZh ? meta.label : meta.enLabel;
}

export function useModuleTitle(key: TabKey): string {
  const { isZh } = useI18n();
  return getModuleTitle(key, isZh);
}
