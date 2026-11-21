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
  readonly description: string;
  readonly defaultKeepDays: number;
  readonly ownerScriptOnly: boolean;
}

export const RETENTION_TARGET_META: readonly RetentionTargetMeta[] = [
  {
    target: "METRIC_SERIES",
    label: "节点原始时序点 (METRIC_SERIES)",
    description:
      "删除超期原始时序点，并由调度器在整月分区早于所有租户最大保留期时安全卸载分区",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "METRIC_ROLLUP_MINUTE",
    label: "分钟级指标汇总 (METRIC_ROLLUP_MINUTE)",
    description: "清理过期的 MINUTE 粒度预聚合汇总点，释放高频时序存储空间",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "AUDIT",
    label: "防篡改审计日志 (AUDIT)",
    description:
      "应用角色无删除权限，由属主定时脚本 scripts/ops/retention-audit.sh 执行并推进 anchor_id / anchor_hash 哈希链锚点",
    defaultKeepDays: 365,
    ownerScriptOnly: true,
  },
  {
    target: "EXPORT_FILE",
    label: "出品导出文件 (EXPORT_FILE)",
    description: "删除过期物理导出文件并将对应 xv_export_jobs 状态置为 EXPIRED",
    defaultKeepDays: 7,
    ownerScriptOnly: false,
  },
  {
    target: "FLOW_STEP_LOG",
    label: "流程算子日志 (FLOW_STEP_LOG)",
    description: "清理超期的 ax_flow_step_logs 流水线步骤执行明细日志",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "JOB",
    label: "后台任务记录 (JOB)",
    description: "清理超期且已处于终态（SUCCEEDED / FAILED / CANCELED / EXPIRED）的 ax_jobs 记录",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "OUTBOX",
    label: "事务外发事件 (OUTBOX)",
    description: "清理已投递消费完成且超过保留期的 ax_outbox 事件记录",
    defaultKeepDays: 14,
    ownerScriptOnly: false,
  },
  {
    target: "LAKE_SNAPSHOT",
    label: "DuckLake 历史快照 (LAKE_SNAPSHOT)",
    description:
      "调用解析侧 /lake/maintenance 执行 ducklake_expire_snapshots 与未引用数据文件回收",
    defaultKeepDays: 30,
    ownerScriptOnly: false,
  },
  {
    target: "MESSAGE",
    label: "智能体对话消息 (MESSAGE)",
    description: "清理超期的 ax_messages 对话轮次与上下文消息记录",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "QUERY_HISTORY",
    label: "分析与问数历史 (QUERY_HISTORY)",
    description: "清理超期的 an_query_history SQL 执行记录与 an_ask_turns 问数轮次",
    defaultKeepDays: 90,
    ownerScriptOnly: false,
  },
  {
    target: "CONVERSATION",
    label: "智能体会话归档 (CONVERSATION)",
    description: "清理超期未活跃的 ax_conversations 智能体会话主记录",
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
  PENDING: "排队待执行",
  RUNNING: "运行中",
  SUCCEEDED: "已成功",
  FAILED: "执行失败",
  CANCELED: "已取消",
  EXPIRED: "已过期",
};

export const BACKUP_KIND_OPTIONS: readonly {
  readonly kind: BackupKind;
  readonly label: string;
  readonly description: string;
}[] = [
  {
    kind: "FULL",
    label: "全量一致性备份 (FULL)",
    description: "先执行 PostgreSQL 目录库转储，再增量硬链接同步 DuckLake 数据文件与三维/平面资产",
  },
  {
    kind: "DATABASE",
    label: "仅数据库转储 (DATABASE)",
    description: "使用 pg_dump 目录格式并行备份全部元数据、图谱、策略与湖目录表",
  },
  {
    kind: "LAKE",
    label: "仅湖与资产文件 (LAKE)",
    description: "使用 rsync --link-dest 同步只追加湖数据文件与空间资产目录",
  },
];

export const BACKUP_STATUS_LABELS: Record<BackupStatus, string> = {
  PENDING: "待认领 (PENDING)",
  RUNNING: "备份中 (RUNNING)",
  SUCCEEDED: "已完成 (SUCCEEDED)",
  FAILED: "失败 (FAILED)",
  VERIFIED: "演练已验真 (VERIFIED)",
};

export const TEMPLATE_CONFLICT_OPTIONS: readonly {
  readonly value: TemplateConflictStrategy;
  readonly label: string;
  readonly description: string;
}[] = [
  {
    value: "RENAME",
    label: "自动重命名 (RENAME)",
    description: "遇到同名节点类型、数据集、指标、流程或驾驶舱时追加 _<pack_id> 后缀创建",
  },
  {
    value: "SKIP",
    label: "跳过同名对象 (SKIP)",
    description: "遇到已存在的同名对象时保持现有资产不变并跳过创建",
  },
];

export const TEMPLATE_INSTALL_STATUS_LABELS: Record<
  TemplateInstallStatus,
  string
> = {
  PLANNED: "已规划待安装",
  INSTALLING: "安装执行中",
  INSTALLED: "已安装就绪",
  FAILED: "中断可续装",
  UNINSTALLED: "已安全卸载",
};

export interface DemoScenarioSpec {
  readonly scenario: DemoScenario;
  readonly title: string;
  readonly industry: string;
  readonly nodeTypes: readonly string[];
  readonly highlights: string;
  readonly defaultDashboardSlug: string;
}

export const DEMO_SCENARIO_SPECS: readonly DemoScenarioSpec[] = [
  {
    scenario: "supply_chain",
    title: "供应链协同与断供推演",
    industry: "制造 / 流通",
    nodeTypes: ["Factory", "Warehouse", "Order", "Supplier"],
    highlights:
      "装载产能—库存—履约三层网络、30 日时序回放、低库存告警、F-1 断供推演场景与全景驾驶舱",
    defaultDashboardSlug: "supply-overview",
  },
  {
    scenario: "manufacturing_equipment",
    title: "离散制造设备健康与 OEE",
    industry: "高端装备 / 智造",
    nodeTypes: ["Line", "Machine", "Sensor", "WorkOrder"],
    highlights:
      "装载设备温度/振动高频时序回放、OEE 综合效率派生指标、超温告警转工单与产线平面图驾驶舱",
    defaultDashboardSlug: "manufacturing-overview",
  },
  {
    scenario: "retail_store",
    title: "连锁零售门店与促销推演",
    industry: "零售 / 快消",
    nodeTypes: ["Region", "Store", "Sku", "Promotion"],
    highlights:
      "装载区域—门店—单品销售与库存网络、促销弹性推演场景、门店经营驾驶舱与销售日报 DOCX 模板",
    defaultDashboardSlug: "retail-overview",
  },
];

export const WORK_ITEM_STATUS_COLUMNS: readonly {
  readonly status: WorkItemStatus;
  readonly label: string;
  readonly accentClass: string;
}[] = [
  {
    status: "OPEN",
    label: "待处理 (OPEN)",
    accentClass: "border-xblue/40 bg-xblue-soft text-xblue",
  },
  {
    status: "IN_PROGRESS",
    label: "处理中 (IN_PROGRESS)",
    accentClass:
      "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  },
  {
    status: "BLOCKED",
    label: "已阻塞 (BLOCKED)",
    accentClass: "border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400",
  },
  {
    status: "DONE",
    label: "已完成 (DONE)",
    accentClass:
      "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  },
  {
    status: "CANCELED",
    label: "已取消 (CANCELED)",
    accentClass: "border-border bg-muted text-muted-foreground",
  },
];

export const WORK_ITEM_KIND_LABELS: Record<WorkItemKind, string> = {
  TASK: "常规任务 (TASK)",
  ISSUE: "异常问题 (ISSUE)",
  CHANGE: "变更交付 (CHANGE)",
};

export const WORK_ITEM_PRIORITY_LABELS: Record<WorkItemPriority, string> = {
  P0: "P0 · 紧急阻断",
  P1: "P1 · 高优处置",
  P2: "P2 · 常规计划",
  P3: "P3 · 低优待办",
};

export const WORK_ITEM_SOURCE_LABELS: Record<WorkItemSourceKind, string> = {
  MANUAL: "手动创建",
  FLOW_ACTION: "流程动作 (ACTION_EMIT)",
  ALERT: "告警转入",
  SIM_DECISION: "推演决策",
  TEMPLATE: "模板待办",
};

export const WORK_ITEM_EVENT_LABELS: Record<WorkItemEventKind, string> = {
  CREATED: "创建工作项 (CREATED)",
  STATUS: "状态流转 (STATUS)",
  ASSIGN: "指派变更 (ASSIGN)",
  COMMENT: "协作备注 (COMMENT)",
  LINK: "关联事件回写 (LINK)",
};
