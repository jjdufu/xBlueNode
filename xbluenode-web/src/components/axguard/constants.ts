import type { AuditAction, Permission, PiiCategory } from "@/lib/api/enums";
import type {
  GrantResourceKind,
  GrantStatus,
  GranteeKind,
  MaskRule,
  Sensitivity,
} from "@/lib/api/axguard";

export interface PermissionDomainGroup {
  readonly domain: string;
  readonly enDomain: string;
  readonly description: string;
  readonly enDescription: string;
  readonly permissions: ReadonlyArray<{
    readonly code: Permission;
    readonly label: string;
  }>;
}

export const PERMISSION_DOMAIN_GROUPS: readonly PermissionDomainGroup[] = [
  {
    domain: "数据与分析",
    enDomain: "Data & Analytics",
    description: "数据目录读取、连接器同步、湖仓管理、全格式出品与敏感明文查阅",
    enDescription: "Catalog access, connector sync, lakehouse admin, exports, and raw PII access",
    permissions: [
      { code: "DATA_READ", label: "数据读取" },
      { code: "DATA_SYNC", label: "数据同步" },
      { code: "DATA_ADMIN", label: "数据连接管理" },
      { code: "EXPORT_DATA", label: "数据出品导出" },
      { code: "PII_RAW_READ", label: "敏感原文调阅" },
    ],
  },
  {
    domain: "图谱与仿真",
    enDomain: "Graph & Simulation",
    description: "多子图节点建模发布、写时复制因果推演与孪生驾驶舱构建分享",
    enDescription: "Subgraph ontology modeling, copy-on-write simulation, and twin dashboards",
    permissions: [
      { code: "STUDIO_EDIT", label: "建模草稿编辑" },
      { code: "STUDIO_PUBLISH", label: "模型版本发布" },
      { code: "SIM_RUN", label: "因果沙盘推演" },
      { code: "SIM_APPLY", label: "推演决策下发" },
      { code: "VIEW_EDIT", label: "驾驶舱编辑" },
      { code: "VIEW_SHARE", label: "驾驶舱外链分享" },
    ],
  },
  {
    domain: "编排与智能体",
    enDomain: "Flows, Alerts & Agents",
    description: "流水线编排运行、人工审批门裁决、异常规则告警与多智能体调用",
    enDescription: "Pipeline orchestration, human approvals, alert rules, and multi-agent execution",
    permissions: [
      { code: "FLOW_EDIT", label: "流程编排编辑" },
      { code: "FLOW_RUN", label: "流程触发执行" },
      { code: "FLOW_PUBLISH", label: "流程生产部署" },
      { code: "APPROVAL_DECIDE", label: "人工审批裁决" },
      { code: "RULE_EDIT", label: "监控规则配置" },
      { code: "ALERT_ACK", label: "告警确认处置" },
      { code: "AGENT_EDIT", label: "智能体配置" },
      { code: "AGENT_RUN", label: "智能体会话" },
    ],
  },
  {
    domain: "安全与运维",
    enDomain: "Security & Operations",
    description: "身份权限治理、ABAC 与行列脱敏策略、审计验真与平台备份运维",
    enDescription: "Identity governance, ABAC & masking policies, audit verification, and operations",
    permissions: [
      { code: "GUARD_ADMIN", label: "安全策略治理" },
      { code: "OPS_ADMIN", label: "系统运维管理" },
    ],
  },
];

export const ALL_PERMISSIONS: readonly Permission[] =
  PERMISSION_DOMAIN_GROUPS.flatMap((g) => g.permissions.map((p) => p.code));

export const SENSITIVITY_OPTIONS: ReadonlyArray<{
  readonly value: Sensitivity;
  readonly label: string;
  readonly enLabel: string;
  readonly level: number;
}> = [
  { value: "PUBLIC", label: "公开 (L0)", enLabel: "Public (L0)", level: 0 },
  { value: "INTERNAL", label: "内部 (L1)", enLabel: "Internal (L1)", level: 1 },
  {
    value: "CONFIDENTIAL",
    label: "机密 (L2)",
    enLabel: "Confidential (L2)",
    level: 2,
  },
  { value: "SECRET", label: "绝密 (L3)", enLabel: "Secret (L3)", level: 3 },
];

export const SENSITIVITY_SHORT_LABELS: Record<Sensitivity, string> = {
  PUBLIC: "公开",
  INTERNAL: "内部",
  CONFIDENTIAL: "机密",
  SECRET: "绝密",
};

export const MASK_RULE_OPTIONS: ReadonlyArray<{
  readonly value: MaskRule;
  readonly label: string;
  readonly enLabel: string;
  readonly sample: string;
}> = [
  {
    value: "NONE",
    label: "不脱敏（显式豁免）",
    enLabel: "Unmasked (Exempt)",
    sample: "13800138000",
  },
  {
    value: "MASK_PHONE",
    label: "手机号脱敏",
    enLabel: "Phone Mask",
    sample: "138****5678",
  },
  {
    value: "MASK_ID_CARD",
    label: "身份证号脱敏",
    enLabel: "ID Card Mask",
    sample: "110101********1234",
  },
  {
    value: "MASK_EMAIL",
    label: "邮箱脱敏",
    enLabel: "Email Mask",
    sample: "z*******@example.com",
  },
  {
    value: "MASK_BANK_CARD",
    label: "银行卡脱敏",
    enLabel: "Bank Card Mask",
    sample: "***************3445",
  },
  {
    value: "MASK_NAME",
    label: "姓名脱敏",
    enLabel: "Name Mask",
    sample: "张* / 欧阳**",
  },
  {
    value: "MASK_ADDRESS",
    label: "地址脱敏",
    enLabel: "Address Mask",
    sample: "上海市浦东新区****",
  },
  {
    value: "HMAC_HASH",
    label: "租户密钥哈希",
    enLabel: "HMAC-SHA256 Hash",
    sample: "9f86d081884c7d65",
  },
  {
    value: "REDACT",
    label: "完全遮蔽",
    enLabel: "Full Redaction",
    sample: "[REDACTED]",
  },
];

export const PII_CATEGORY_LABELS: Record<PiiCategory, string> = {
  NONE: "无敏感特征",
  PHONE: "手机号码",
  ID_CARD: "身份证号",
  EMAIL: "电子邮箱",
  BANK_CARD: "银行卡号",
  NAME: "个人姓名",
  ADDRESS: "详细地址",
  SECRET: "密钥凭据",
};

export const GRANT_RESOURCE_KINDS: ReadonlyArray<{
  readonly value: GrantResourceKind;
  readonly label: string;
  readonly enLabel: string;
}> = [
  { value: "DATASET", label: "数据集", enLabel: "Dataset" },
  { value: "NODE_TYPE", label: "节点类型", enLabel: "Node Type" },
  { value: "SUBGRAPH", label: "业务子图", enLabel: "Subgraph" },
  { value: "DASHBOARD", label: "孪生驾驶舱", enLabel: "Dashboard" },
  { value: "AGENT", label: "智能体应用", enLabel: "Agent" },
];

export const GRANTEE_KINDS: ReadonlyArray<{
  readonly value: GranteeKind;
  readonly label: string;
  readonly enLabel: string;
}> = [
  { value: "DEPT", label: "部门", enLabel: "Department" },
  { value: "ROLE", label: "角色", enLabel: "Role" },
  { value: "USER", label: "用户", enLabel: "User" },
];

export const GRANT_STATUS_LABELS: Record<GrantStatus, string> = {
  PENDING: "待属主审批",
  ACTIVE: "已生效",
  REJECTED: "已驳回",
  REVOKED: "已撤销",
  EXPIRED: "已过期",
};

export const AUDIT_ACTIONS: readonly AuditAction[] = [
  "LOGIN",
  "LOGOUT",
  "TOKEN_ISSUE",
  "TOKEN_REVOKE",
  "QUERY_SQL",
  "ASK_DATA",
  "RAG_ASK",
  "DATA_SYNC",
  "FILE_UPLOAD",
  "SCHEMA_PUBLISH",
  "FLOW_RUN",
  "APPROVAL_DECIDE",
  "MCP_CALL",
  "AGENT_CHAT",
  "SIM_APPLY",
  "EXPORT_FILE",
  "SHARE_CREATE",
  "SHARE_ACCESS",
  "POLICY_CHANGE",
  "BACKUP_RUN",
  "TEMPLATE_INSTALL",
  "BUILDER_MCP_MUTATE",
  "TRIAL_TEMPLATE_CLONE",
  "STUDIO_SNAPSHOT_RESTORE",
  "IM_ACCOUNT_BIND",
  "CHART_PUBLISH",
  "OBJECT_ACTION_INVOKE",
  "TRIAL_TICK_CERTIFY",
];

export interface CedarSnippetTemplate {
  readonly key: string;
  readonly title: string;
  readonly enTitle: string;
  readonly suggestedName: string;
  readonly description: string;
  readonly enDescription: string;
  readonly policyText: string;
}

export const CEDAR_SNIPPET_TEMPLATES: readonly CedarSnippetTemplate[] = [
  {
    key: "work_hours_export",
    title: "非工作时间禁止导出",
    enTitle: "Block Off-Hours Export",
    suggestedName: "forbid_offhours_export",
    description: "每日 20:00 至次日 08:00 禁止执行数据导出",
    enDescription: "Forbid data exports between 20:00 and 08:00",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"EXPORT_DATA",\n  resource\n)\nwhen {\n  context.hour < 8 || context.hour >= 20\n};',
  },
  {
    key: "secret_clearance_guard",
    title: "绝密资源严格封锁",
    enTitle: "Restrict Secret Exports",
    suggestedName: "forbid_secret_export",
    description: "禁止对绝密级资源执行外部导出与分享",
    enDescription: "Forbid external export or sharing of Secret resources",
    policyText:
      'forbid (\n  principal,\n  action in [AxPoint::Action::"EXPORT_DATA", AxPoint::Action::"VIEW_SHARE"],\n  resource\n)\nwhen {\n  resource.sensitivity >= 3\n};',
  },
  {
    key: "ip_allowlist",
    title: "来源内网 IP 限制",
    enTitle: "Internal IP Only for PII",
    suggestedName: "forbid_external_ip_pii",
    description: "非内网 10.* 或本地回环地址禁止调阅敏感明文",
    enDescription: "Restrict raw PII reads to internal 10.* or loopback IPs",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"PII_RAW_READ",\n  resource\n)\nunless {\n  context.ip like "10.*" || context.ip == "127.0.0.1"\n};',
  },
  {
    key: "no_export_all",
    title: "全局禁止数据出品",
    enTitle: "Freeze All Exports",
    suggestedName: "no_export",
    description: "冻结当前租户全部数据导出任务",
    enDescription: "Freeze all data exports across the tenant",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"EXPORT_DATA",\n  resource\n);',
  },
];

export const ROW_POLICY_PLACEHOLDER_CHIPS: ReadonlyArray<{
  readonly token: string;
  readonly hint: string;
}> = [
  { token: "{{user.id}}", hint: "User UUID" },
  { token: "{{user.dept_id}}", hint: "Department ID" },
  { token: "{{user.dept}}", hint: "Department Slug" },
  { token: "{{user.username}}", hint: "Username" },
  { token: "{{user.attr.warehouses}}", hint: "Multi-value User Attribute" },
];
