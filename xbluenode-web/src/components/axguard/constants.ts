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
  readonly description: string;
  readonly permissions: ReadonlyArray<{
    readonly code: Permission;
    readonly label: string;
  }>;
}

export const PERMISSION_DOMAIN_GROUPS: readonly PermissionDomainGroup[] = [
  {
    domain: "数据与分析",
    description: "数据目录读取、连接器同步、湖仓管理、全格式出品与敏感明文查阅",
    permissions: [
      { code: "DATA_READ", label: "数据读取 (DATA_READ)" },
      { code: "DATA_SYNC", label: "数据同步 (DATA_SYNC)" },
      { code: "DATA_ADMIN", label: "数据管理豁免 (DATA_ADMIN)" },
      { code: "EXPORT_DATA", label: "数据出品导出 (EXPORT_DATA)" },
      { code: "PII_RAW_READ", label: "PII 原文调阅 (PII_RAW_READ)" },
    ],
  },
  {
    domain: "图谱与仿真",
    description: "多子图节点建模发布、写时复制因果推演与孪生驾驶舱构建分享",
    permissions: [
      { code: "STUDIO_EDIT", label: "建模草稿编辑 (STUDIO_EDIT)" },
      { code: "STUDIO_PUBLISH", label: "模型版本发布 (STUDIO_PUBLISH)" },
      { code: "SIM_RUN", label: "因果沙盘推演 (SIM_RUN)" },
      { code: "SIM_APPLY", label: "推演决策下发 (SIM_APPLY)" },
      { code: "VIEW_EDIT", label: "驾驶舱编辑 (VIEW_EDIT)" },
      { code: "VIEW_SHARE", label: "安全外链分享 (VIEW_SHARE)" },
    ],
  },
  {
    domain: "编排与智能体",
    description: "流水线编排运行、人工审批门裁决、异常规则告警与多智能体调用",
    permissions: [
      { code: "FLOW_EDIT", label: "流程编排编辑 (FLOW_EDIT)" },
      { code: "FLOW_RUN", label: "流程触发执行 (FLOW_RUN)" },
      { code: "FLOW_PUBLISH", label: "流程环境部署 (FLOW_PUBLISH)" },
      { code: "APPROVAL_DECIDE", label: "人工审批裁决 (APPROVAL_DECIDE)" },
      { code: "RULE_EDIT", label: "监控规则配置 (RULE_EDIT)" },
      { code: "ALERT_ACK", label: "告警认领处置 (ALERT_ACK)" },
      { code: "AGENT_EDIT", label: "智能体构建 (AGENT_EDIT)" },
      { code: "AGENT_RUN", label: "智能体会话 (AGENT_RUN)" },
    ],
  },
  {
    domain: "安全与运维",
    description: "身份权限治理、ABAC 与行列脱敏策略、审计验真与平台备份运维",
    permissions: [
      { code: "GUARD_ADMIN", label: "安全治理管理 (GUARD_ADMIN)" },
      { code: "OPS_ADMIN", label: "平台运维管理 (OPS_ADMIN)" },
    ],
  },
];

export const ALL_PERMISSIONS: readonly Permission[] =
  PERMISSION_DOMAIN_GROUPS.flatMap((g) => g.permissions.map((p) => p.code));

export const SENSITIVITY_OPTIONS: ReadonlyArray<{
  readonly value: Sensitivity;
  readonly label: string;
  readonly level: number;
}> = [
  { value: "PUBLIC", label: "PUBLIC · 公开 (L0)", level: 0 },
  { value: "INTERNAL", label: "INTERNAL · 内部 (L1)", level: 1 },
  { value: "CONFIDENTIAL", label: "CONFIDENTIAL · 机密 (L2)", level: 2 },
  { value: "SECRET", label: "SECRET · 绝密 (L3)", level: 3 },
];

export const SENSITIVITY_SHORT_LABELS: Record<Sensitivity, string> = {
  PUBLIC: "公开 PUBLIC",
  INTERNAL: "内部 INTERNAL",
  CONFIDENTIAL: "机密 CONFIDENTIAL",
  SECRET: "绝密 SECRET",
};

export const MASK_RULE_OPTIONS: ReadonlyArray<{
  readonly value: MaskRule;
  readonly label: string;
  readonly sample: string;
}> = [
  { value: "NONE", label: "NONE · 不脱敏（显式豁免）", sample: "原文直出" },
  { value: "MASK_PHONE", label: "MASK_PHONE · 手机号脱敏", sample: "138****5678" },
  {
    value: "MASK_ID_CARD",
    label: "MASK_ID_CARD · 身份证号脱敏",
    sample: "110101********1234",
  },
  {
    value: "MASK_EMAIL",
    label: "MASK_EMAIL · 邮箱脱敏",
    sample: "z*******@example.com",
  },
  {
    value: "MASK_BANK_CARD",
    label: "MASK_BANK_CARD · 银行卡脱敏",
    sample: "***************3445",
  },
  { value: "MASK_NAME", label: "MASK_NAME · 姓名脱敏", sample: "张* / 欧阳**" },
  {
    value: "MASK_ADDRESS",
    label: "MASK_ADDRESS · 地址脱敏（保留区县）",
    sample: "上海市浦东新区****",
  },
  {
    value: "HMAC_HASH",
    label: "HMAC_HASH · 租户密钥关联哈希",
    sample: "9f86d081884c7d65",
  },
  { value: "REDACT", label: "REDACT · 完全遮蔽", sample: "[REDACTED]" },
];

export const PII_CATEGORY_LABELS: Record<PiiCategory, string> = {
  NONE: "无敏感特征",
  PHONE: "手机号 (PHONE)",
  ID_CARD: "身份证 (ID_CARD)",
  EMAIL: "电子邮箱 (EMAIL)",
  BANK_CARD: "银行卡 (BANK_CARD)",
  NAME: "个人姓名 (NAME)",
  ADDRESS: "详细地址 (ADDRESS)",
  SECRET: "密钥凭据 (SECRET)",
};

export const GRANT_RESOURCE_KINDS: ReadonlyArray<{
  readonly value: GrantResourceKind;
  readonly label: string;
}> = [
  { value: "DATASET", label: "数据集 (DATASET)" },
  { value: "NODE_TYPE", label: "业务节点类型 (NODE_TYPE)" },
  { value: "SUBGRAPH", label: "业务子图 (SUBGRAPH)" },
  { value: "DASHBOARD", label: "孪生驾驶舱 (DASHBOARD)" },
  { value: "AGENT", label: "智能体应用 (AGENT)" },
];

export const GRANTEE_KINDS: ReadonlyArray<{
  readonly value: GranteeKind;
  readonly label: string;
}> = [
  { value: "DEPT", label: "部门 (DEPT)" },
  { value: "ROLE", label: "角色 (ROLE)" },
  { value: "USER", label: "指定用户 (USER)" },
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
];

export interface CedarSnippetTemplate {
  readonly key: string;
  readonly title: string;
  readonly suggestedName: string;
  readonly description: string;
  readonly policyText: string;
}

export const CEDAR_SNIPPET_TEMPLATES: readonly CedarSnippetTemplate[] = [
  {
    key: "work_hours_export",
    title: "非工作时间禁止导出",
    suggestedName: "forbid_offhours_export",
    description: "每日 20:00 至次日 08:00 禁止执行任何数据出品动作",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"EXPORT_DATA",\n  resource\n)\nwhen {\n  context.hour < 8 || context.hour >= 20\n};',
  },
  {
    key: "secret_clearance_guard",
    title: "绝密资源严格封锁",
    suggestedName: "forbid_secret_export",
    description: "禁止对绝密级（sensitivity == 3）资源执行外部导出与分享",
    policyText:
      'forbid (\n  principal,\n  action in [AxPoint::Action::"EXPORT_DATA", AxPoint::Action::"VIEW_SHARE"],\n  resource\n)\nwhen {\n  resource.sensitivity >= 3\n};',
  },
  {
    key: "ip_allowlist",
    title: "来源内网 IP 限制",
    suggestedName: "forbid_external_ip_pii",
    description: "非内网 10.* 或本地回环地址禁止调阅敏感明文",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"PII_RAW_READ",\n  resource\n)\nunless {\n  context.ip like "10.*" || context.ip == "127.0.0.1"\n};',
  },
  {
    key: "no_export_all",
    title: "全局禁止数据出品",
    suggestedName: "no_export",
    description: "紧急冻结当前租户全部数据出品任务",
    policyText:
      'forbid (\n  principal,\n  action == AxPoint::Action::"EXPORT_DATA",\n  resource\n);',
  },
];

export const ROW_POLICY_PLACEHOLDER_CHIPS: ReadonlyArray<{
  readonly token: string;
  readonly hint: string;
}> = [
  { token: "{{user.id}}", hint: "当前用户 UUID" },
  { token: "{{user.dept_id}}", hint: "用户部门 ID" },
  { token: "{{user.dept}}", hint: "部门标识 (§13.6)" },
  { token: "{{user.username}}", hint: "当前登录账号名" },
  { token: "{{user.attr.warehouses}}", hint: "用户多值属性 (用于 IN 右侧)" },
];
