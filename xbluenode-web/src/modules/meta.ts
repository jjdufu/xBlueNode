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
  | "axguard"
  | "axops";

export type NavGroupKey =
  | "foundation"
  | "analytics"
  | "modeling"
  | "governance";

export interface NavGroupMeta {
  readonly key: NavGroupKey;
  readonly zh: string;
  readonly en: string;
}

export const NAV_GROUPS: readonly NavGroupMeta[] = [
  { key: "foundation", zh: "数据底座", en: "Data Foundation" },
  { key: "analytics", zh: "分析洞察", en: "Analytics & Insight" },
  { key: "modeling", zh: "建模与编排", en: "Modeling & AI" },
  { key: "governance", zh: "推演与治理", en: "Simulation & Ops" },
];

export interface ModuleTitleMeta {
  readonly key: TabKey;
  readonly productTag?: string;
  readonly shortZh: string;
  readonly shortEn: string;
  readonly label: string;
  readonly enLabel: string;
  readonly group?: NavGroupKey;
}

/**
 * Single Source of Truth (G1) for sidebar navigation labels and page H1 titles.
 */
export const MODULE_META: Readonly<Record<TabKey, ModuleTitleMeta>> = {
  dashboard: {
    key: "dashboard",
    shortZh: "系统概览",
    shortEn: "Overview",
    label: "系统概览",
    enLabel: "Overview",
    group: "foundation",
  },
  axhub: {
    key: "axhub",
    productTag: "AxHub",
    shortZh: "数据接入与目录",
    shortEn: "Data Fabric",
    label: "AxHub · 数据接入与目录",
    enLabel: "AxHub · Data Fabric",
    group: "foundation",
  },
  axingest: {
    key: "axingest",
    productTag: "AxIngest",
    shortZh: "知识入库",
    shortEn: "Ingestion",
    label: "AxIngest · 知识入库",
    enLabel: "AxIngest · Ingestion",
    group: "foundation",
  },
  axanalytics: {
    key: "axanalytics",
    productTag: "AxAnalytics",
    shortZh: "分析与问数",
    shortEn: "Smart BI",
    label: "AxAnalytics · 分析与问数",
    enLabel: "AxAnalytics · Smart BI",
    group: "analytics",
  },
  axquery: {
    key: "axquery",
    productTag: "AxQuery",
    shortZh: "智能问答",
    shortEn: "Hybrid Q&A",
    label: "AxQuery · 智能问答",
    enLabel: "AxQuery · Hybrid Q&A",
    group: "analytics",
  },
  xblueview: {
    key: "xblueview",
    productTag: "xBlueView",
    shortZh: "孪生驾驶舱",
    shortEn: "Twin Cockpit",
    label: "xBlueView · 孪生驾驶舱",
    enLabel: "xBlueView · Twin Cockpit",
    group: "analytics",
  },
  xbluegraph: {
    key: "xbluegraph",
    productTag: "xBlueGraph",
    shortZh: "图浏览",
    shortEn: "Explorer",
    label: "xBlueGraph · 图浏览",
    enLabel: "xBlueGraph · Explorer",
    group: "modeling",
  },
  xbluestudio: {
    key: "xbluestudio",
    productTag: "xBlueStudio",
    shortZh: "节点建模",
    shortEn: "Ontology",
    label: "xBlueStudio · 节点建模",
    enLabel: "xBlueStudio · Ontology",
    group: "modeling",
  },
  axflow: {
    key: "axflow",
    productTag: "AxFlow",
    shortZh: "编排",
    shortEn: "Pipelines",
    label: "AxFlow · 编排",
    enLabel: "AxFlow · Pipelines",
    group: "modeling",
  },
  axagent: {
    key: "axagent",
    productTag: "AxAgent",
    shortZh: "智能体与发布",
    shortEn: "Agents & API",
    label: "AxAgent · 智能体与发布",
    enLabel: "AxAgent · Agents & API",
    group: "modeling",
  },
  axsim: {
    key: "axsim",
    productTag: "AxSim",
    shortZh: "因果推演",
    shortEn: "Simulation",
    label: "AxSim · 因果推演",
    enLabel: "AxSim · Simulation",
    group: "governance",
  },
  axalerts: {
    key: "axalerts",
    productTag: "AxAlerts",
    shortZh: "告警中心",
    shortEn: "Monitoring",
    label: "AxAlerts · 告警中心",
    enLabel: "AxAlerts · Monitoring",
    group: "governance",
  },
  axguard: {
    key: "axguard",
    productTag: "AxGuard",
    shortZh: "安全治理",
    shortEn: "Governance",
    label: "AxGuard · 安全治理",
    enLabel: "AxGuard · Governance",
    group: "governance",
  },
  axops: {
    key: "axops",
    productTag: "AxOps",
    shortZh: "运维与交付",
    shortEn: "Operations",
    label: "AxOps · 运维与交付",
    enLabel: "AxOps · Operations",
    group: "governance",
  },
  agent_embed: {
    key: "agent_embed",
    productTag: "AxAgent",
    shortZh: "嵌入对话",
    shortEn: "Assistant",
    label: "AxAgent · 智能对话",
    enLabel: "AxAgent · Assistant",
  },
  xblueview_share: {
    key: "xblueview_share",
    productTag: "xBlueView",
    shortZh: "安全分享",
    shortEn: "Share",
    label: "xBlueView · 安全分享",
    enLabel: "xBlueView · Share",
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
