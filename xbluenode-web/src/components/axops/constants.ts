import type {
  BackupKind,
  BackupStatus,
  DemoScenario,
  TemplateConflictStrategy,
  TemplateInstallStatus,
  WorkItemEventKind,
  WorkItemKind,
  WorkItemPriority,
  WorkItemSourceKind,
  WorkItemStatus,
} from "../../lib/api/axops.ts";
import type {
  JobKind,
  JobStatus,
  RetentionTarget,
} from "../../lib/api/enums.ts";

export type AxOpsTabKey =
  | "health"
  | "jobs"
  | "retention"
  | "backups"
  | "templates"
  | "work_items";

export interface RetentionTargetMeta {
  readonly target: RetentionTarget;
  readonly label: string;
  readonly enLabel: string;
  readonly description: string;
  readonly enDescription: string;
  readonly defaultKeepDays: number;
  readonly ownerScriptOnly: boolean;
}

export const RETENTION_TARGET_META: readonly RetentionTargetMeta[] = [
  {
    target: "METRIC_SERIES",
    label: "节点原始时序点",
    enLabel: "Raw Node Metric Series",
    description:
      "回收超期原始时序点，并在整月分区早于全部租户最大保留期时自动卸载分区",
    enDescription:
      "Purges expired raw metric points and detaches monthly partitions past all tenant retention windows",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "METRIC_ROLLUP_MINUTE",
    label: "分钟级指标汇总",
    enLabel: "Minute Metric Rollups",
    description: "清理过期的分钟粒度预聚合汇总点，释放高频时序存储空间",
    enDescription:
      "Purges expired minute-level pre-aggregated rollup points to reclaim time-series storage",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "AUDIT",
    label: "防篡改审计日志",
    enLabel: "Tamper-Evident Audit Logs",
    description:
      "应用角色无删除权限，由属主定时脚本 retention-audit.sh 执行并推进哈希链锚点",
    enDescription:
      "App role has no DELETE privilege; executed by owner script retention-audit.sh while advancing hash-chain anchors",
    defaultKeepDays: 365,
    ownerScriptOnly: true,
  },
  {
    target: "EXPORT_FILE",
    label: "出品导出文件",
    enLabel: "Exported Artifacts",
    description: "删除过期物理导出文件并将对应导出任务状态标记为已过期",
    enDescription:
      "Deletes expired physical export files and marks corresponding export jobs as EXPIRED",
    defaultKeepDays: 7,
    ownerScriptOnly: false,
  },
  {
    target: "FLOW_STEP_LOG",
    label: "流程算子日志",
    enLabel: "Flow Step Logs",
    description: "清理超期的流水线步骤执行明细日志",
    enDescription: "Purges expired pipeline step execution logs",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "JOB",
    label: "后台任务记录",
    enLabel: "Background Job Records",
    description: "清理超期且已处于终态的异步任务记录",
    enDescription:
      "Purges expired background job records in terminal states (SUCCEEDED / FAILED / CANCELED / EXPIRED)",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "OUTBOX",
    label: "事务外发事件",
    enLabel: "Transactional Outbox Events",
    description: "清理已投递消费完成且超过保留期的事务外发记录",
    enDescription:
      "Purges delivered transactional outbox events past the retention period",
    defaultKeepDays: 14,
    ownerScriptOnly: false,
  },
  {
    target: "LAKE_SNAPSHOT",
    label: "分析湖历史快照",
    enLabel: "DuckLake Snapshots",
    description: "调用湖维护接口过期历史快照并回收未引用数据文件",
    enDescription:
      "Expires historical DuckLake snapshots and reclaims unreferenced data files",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "MESSAGE",
    label: "智能体对话消息",
    enLabel: "Agent Messages",
    description: "清理超期的智能体对话轮次与上下文消息记录",
    enDescription: "Purges expired agent conversation turns and message records",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "QUERY_HISTORY",
    label: "分析与问数历史",
    enLabel: "Query & Ask History",
    description: "清理超期的 SQL 执行历史与智能问数轮次记录",
    enDescription: "Purges expired SQL execution history and NL2SQL ask turns",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "CONVERSATION",
    label: "智能体会话归档",
    enLabel: "Agent Conversations",
    description: "清理超期未活跃的智能体会话主记录",
    enDescription: "Purges inactive agent conversation records past retention",
    defaultKeepDays: 180,
    ownerScriptOnly: false,
  },
];

export const ALL_JOB_KINDS: readonly JobKind[] = [
  "SYNC_DATASET",
  "PARSE_FILE",
  "FLOW_STEP",
  "EXPORT",
  "RULE_EVAL",
  "ROLLUP",
  "LAKE_MAINTENANCE",
  "RETENTION",
  "CREDENTIAL_ROTATE",
  "BACKUP",
  "EMBED_NODES",
  "MAP_MATERIALIZE",
  "GRAPH_AGGREGATE",
  "KNOWLEDGE_SYNC",
];

export const JOB_KIND_LABELS: Record<JobKind, string> = {
  SYNC_DATASET: "数据集同步",
  PARSE_FILE: "文件双轨解析",
  FLOW_STEP: "编排算子步",
  EXPORT: "出品文件渲染",
  RULE_EVAL: "告警规则判定",
  ROLLUP: "时序指标汇总",
  LAKE_MAINTENANCE: "湖快照维护",
  RETENTION: "保留策略清理",
  CREDENTIAL_ROTATE: "凭据密钥轮换",
  BACKUP: "平台备份任务",
  EMBED_NODES: "节点向量化",
  MAP_MATERIALIZE: "图映射物化",
  GRAPH_AGGREGATE: "图聚合重算",
  KNOWLEDGE_SYNC: "知识库同步",
};

export const ALL_JOB_STATUSES: readonly JobStatus[] = [
  "PENDING",
  "RUNNING",
  "SUCCEEDED",
  "FAILED",
  "CANCELED",
  "EXPIRED",
];

export const JOB_STATUS_LABELS: Record<JobStatus, string> = {
  PENDING: "排队中",
  RUNNING: "运行中",
  SUCCEEDED: "成功",
  FAILED: "失败",
  CANCELED: "已取消",
  EXPIRED: "已过期",
};

export const BACKUP_KIND_OPTIONS: readonly {
  readonly kind: BackupKind;
  readonly label: string;
  readonly enLabel: string;
  readonly description: string;
  readonly enDescription: string;
}[] = [
  {
    kind: "FULL",
    label: "全量一致性备份",
    enLabel: "Full Consistent Backup",
    description:
      "依次执行 PostgreSQL 目录库转储与 DuckLake 数据及空间资产增量硬链接同步",
    enDescription:
      "Runs PostgreSQL directory dump followed by hard-link incremental sync of DuckLake and spatial assets",
  },
  {
    kind: "DATABASE",
    label: "数据库转储",
    enLabel: "Database Only",
    description: "并行备份全部元数据、图谱、策略与湖目录表",
    enDescription:
      "Parallel pg_dump directory backup of metadata, graph, policies, and lake catalog tables",
  },
  {
    kind: "LAKE",
    label: "分析湖与资产文件",
    enLabel: "Lake & Assets Only",
    description: "增量硬链接同步只追加湖数据文件与空间资产目录",
    enDescription:
      "Hard-link incremental sync of append-only lake Parquet files and spatial asset directories",
  },
];

export const BACKUP_STATUS_LABELS: Record<BackupStatus, string> = {
  PENDING: "待执行",
  RUNNING: "备份中",
  SUCCEEDED: "已完成",
  FAILED: "失败",
  VERIFIED: "已验真",
};

export const TEMPLATE_CONFLICT_OPTIONS: readonly {
  readonly value: TemplateConflictStrategy;
  readonly label: string;
  readonly enLabel: string;
  readonly description: string;
  readonly enDescription: string;
}[] = [
  {
    value: "RENAME",
    label: "自动重命名",
    enLabel: "Auto Rename",
    description: "遇到同名资产时追加模板包后缀创建",
    enDescription: "Appends _<pack_id> suffix when a same-named asset exists",
  },
  {
    value: "SKIP",
    label: "跳过同名对象",
    enLabel: "Skip Existing",
    description: "保留现有同名资产不变并跳过创建",
    enDescription: "Keeps existing assets untouched and skips conflicting items",
  },
];

export const TEMPLATE_INSTALL_STATUS_LABELS: Record<
  TemplateInstallStatus,
  string
> = {
  PLANNED: "待安装",
  INSTALLING: "安装中",
  INSTALLED: "已安装",
  FAILED: "可续装",
  UNINSTALLED: "已卸载",
};

export interface DemoScenarioSpec {
  readonly scenario: DemoScenario;
  readonly title: string;
  readonly enTitle: string;
  readonly industry: string;
  readonly enIndustry: string;
  readonly nodeTypes: readonly string[];
  readonly highlights: string;
  readonly enHighlights: string;
  readonly defaultDashboardSlug: string;
}

export const DEMO_SCENARIO_SPECS: readonly DemoScenarioSpec[] = [
  {
    scenario: "supply_chain",
    title: "供应链协同与断供推演",
    enTitle: "Supply Chain & Disruption Sim",
    industry: "制造 / 流通",
    enIndustry: "Manufacturing / Logistics",
    nodeTypes: ["Factory", "Warehouse", "Order", "Supplier"],
    highlights:
      "装载产能—库存—履约三层网络、30 日时序回放、低库存告警、断供推演场景与全景驾驶舱",
    enHighlights:
      "Loads Factory–Warehouse–Order network, 30-day metric replay, low-stock alerts, disruption scenario, and executive dashboard",
    defaultDashboardSlug: "supply-overview",
  },
  {
    scenario: "manufacturing_equipment",
    title: "离散制造设备健康与 OEE",
    enTitle: "Equipment Health & OEE",
    industry: "高端装备 / 智造",
    enIndustry: "Discrete Manufacturing",
    nodeTypes: ["Line", "Machine", "Sensor", "WorkOrder"],
    highlights:
      "装载设备温度与振动高频时序回放、OEE 派生指标、超温告警转工单与产线平面图驾驶舱",
    enHighlights:
      "Loads machine temperature/vibration telemetry, derived OEE metrics, overheat alert-to-workorder flow, and floor-plan dashboard",
    defaultDashboardSlug: "manufacturing-overview",
  },
  {
    scenario: "retail_store",
    title: "连锁零售门店与促销推演",
    enTitle: "Retail Store & Promo Sim",
    industry: "零售 / 快消",
    enIndustry: "Retail / CPG",
    nodeTypes: ["Region", "Store", "Sku", "Promotion"],
    highlights:
      "装载区域—门店—单品销售与库存网络、促销弹性推演场景、门店经营驾驶舱与销售日报模板",
    enHighlights:
      "Loads Region–Store–SKU network, promotion elasticity scenarios, store operations dashboard, and daily DOCX report template",
    defaultDashboardSlug: "retail-overview",
  },
];

export const WORK_ITEM_STATUS_COLUMNS: readonly {
  readonly status: WorkItemStatus;
  readonly label: string;
  readonly enLabel: string;
  readonly accentClass: string;
}[] = [
  {
    status: "OPEN",
    label: "待处理",
    enLabel: "Open",
    accentClass: "border-xblue/40 bg-xblue-soft text-xblue",
  },
  {
    status: "IN_PROGRESS",
    label: "处理中",
    enLabel: "In Progress",
    accentClass:
      "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  },
  {
    status: "BLOCKED",
    label: "已阻塞",
    enLabel: "Blocked",
    accentClass:
      "border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400",
  },
  {
    status: "DONE",
    label: "已完成",
    enLabel: "Done",
    accentClass:
      "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  },
  {
    status: "CANCELED",
    label: "已取消",
    enLabel: "Canceled",
    accentClass: "border-border bg-muted text-muted-foreground",
  },
];

export const WORK_ITEM_KIND_LABELS: Record<WorkItemKind, string> = {
  TASK: "常规任务",
  ISSUE: "异常问题",
  CHANGE: "变更交付",
};

export const WORK_ITEM_PRIORITY_LABELS: Record<WorkItemPriority, string> = {
  P0: "P0 · 紧急",
  P1: "P1 · 高优",
  P2: "P2 · 常规",
  P3: "P3 · 低优",
};

export const WORK_ITEM_SOURCE_LABELS: Record<WorkItemSourceKind, string> = {
  MANUAL: "手动创建",
  FLOW_ACTION: "流程动作",
  ALERT: "告警转入",
  SIM_DECISION: "推演决策",
  TEMPLATE: "模板待办",
};

export const WORK_ITEM_EVENT_LABELS: Record<WorkItemEventKind, string> = {
  CREATED: "创建工作项",
  STATUS: "状态流转",
  ASSIGN: "指派变更",
  COMMENT: "协作备注",
  LINK: "关联回写",
};
