import { z } from "zod";
import { requestJson } from "./client";
import type {
  Environment,
  FlowOperator,
  OutboxTopic,
  RunStatus,
  StepStatus,
  TriggerType,
} from "./enums";

// ============================================================================
// Step 07: AxFlow DAG Orchestration, Versions, Runs, Approvals & Action Logs
// ============================================================================

export type BackoffStrategy = "FIXED" | "EXPONENTIAL";
export type OnFailure = "FAIL_RUN" | "CONTINUE";
export type JoinMode = "ALL_SUCCESS" | "ALL_DONE" | "ANY_SUCCESS";
export type FlowIssueSeverity = "ERROR" | "WARNING";

export type ApprovalSourceKind = "FLOW" | "SIM_APPLY" | "AGENT";
export type ApprovalStatus = "PENDING" | "APPROVED" | "REJECTED" | "EXPIRED";
export type ApprovalDecision = "APPROVE" | "REJECT";

export type ActionSourceKind = "FLOW" | "SIM_APPLY" | "AGENT" | "RULE" | "VIEW";
export type ActionKind =
  | "WRITE_NODE_METRIC"
  | "WRITE_NODE_PROPERTY"
  | "HTTP"
  | "NOTIFY"
  | "WORK_ITEM";
export type ActionLogStatus = "DONE" | "DRY_RUN" | "FAILED";

export interface FlowPos {
  x: number;
  y: number;
}

export interface RetryPolicy {
  max_attempts: number;
  backoff_s: number;
  backoff: BackoffStrategy;
}

export interface FlowNodeDef {
  id: string;
  kind: FlowOperator;
  label: string;
  config: Record<string, unknown>;
  retry: RetryPolicy;
  timeout_s: number;
  on_failure: OnFailure;
  join: JoinMode;
  pos: FlowPos;
}

export interface FlowEdgeDef {
  from: string;
  to: string;
  when?: string | null;
}

export interface FlowGraphDef {
  nodes: FlowNodeDef[];
  edges: FlowEdgeDef[];
}

export interface TriggerCfg {
  type: TriggerType;
  cron?: string | null;
  timezone?: string | null;
  catch_up?: boolean | null;
  topic?: OutboxTopic | string | null;
  filter?: string | null;
}

export interface FlowIssue {
  code: string;
  message: string;
  severity: FlowIssueSeverity;
  node_id?: string | null;
}

export interface ValidateFlowResponse {
  valid: boolean;
  issues: FlowIssue[];
}

export interface FlowRecord {
  id: string;
  name: string;
  description: string;
  slug?: string | null;
  nodes: FlowNodeDef[];
  edges: FlowEdgeDef[];
  trigger_cfg: TriggerCfg;
  status: string;
  created_by?: string | null;
  last_run_at: string | null;
  created_at: string;
  updated_at: string;
  issues: FlowIssue[];
}

export interface FlowPaginatedList<T> {
  items: T[];
  next_cursor: string | null;
}

export interface FlowListQuery {
  q?: string;
  limit?: number;
  cursor?: string;
}

export interface CreateFlowRequest {
  name: string;
  description?: string;
  slug?: string;
  nodes?: FlowNodeDef[];
  edges?: FlowEdgeDef[];
  trigger_cfg?: TriggerCfg;
}

export interface UpdateFlowRequest {
  name?: string;
  description?: string;
  slug?: string;
  nodes?: FlowNodeDef[];
  edges?: FlowEdgeDef[];
  trigger_cfg?: TriggerCfg;
}

export interface FlowVersionRecord {
  id: string;
  flow_id: string;
  version: number;
  nodes: FlowNodeDef[];
  edges: FlowEdgeDef[];
  trigger_cfg: TriggerCfg;
  note: string;
  published_by: string | null;
  published_at: string;
}

export interface FlowDeploymentRecord {
  flow_id: string;
  environment: Environment;
  version: number;
  deployed_by: string | null;
  deployed_at: string;
}

export interface FlowVersionsListResponse {
  items: FlowVersionRecord[];
  deployments: FlowDeploymentRecord[];
  next_cursor: string | null;
}

export interface PublishFlowVersionRequest {
  note?: string;
}

export interface DeployFlowRequest {
  environment: Environment;
  version: number;
}

export interface StartFlowRunRequest {
  environment?: Environment;
  version?: number;
  payload?: Record<string, unknown>;
  idempotency_key?: string;
}

export interface StartFlowRunResponse {
  run_id: string;
  status: RunStatus;
  existing: boolean;
}

export interface FlowRunRecord {
  id: string;
  flow_id: string;
  version: number;
  environment: Environment;
  trigger_source: TriggerType;
  trigger_payload: Record<string, unknown>;
  idempotency_key: string | null;
  status: RunStatus;
  outputs: Record<string, unknown>;
  error_message: string | null;
  started_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface FlowRunListQuery {
  environment?: Environment;
  status?: RunStatus;
  limit?: number;
  cursor?: string;
}

export interface FlowStepRecord {
  id: number;
  run_id: string;
  node_id: string;
  attempt: number;
  status: StepStatus;
  job_id: string | null;
  input: Record<string, unknown>;
  output: unknown;
  output_ref: string | null;
  error_code: string | null;
  error_message: string | null;
  started_at: string | null;
  finished_at: string | null;
}

export interface ApprovalRecord {
  id: string;
  run_id: string | null;
  node_id: string | null;
  source_kind: ApprovalSourceKind;
  source_ref: string;
  title: string;
  detail: Record<string, unknown>;
  assignee_roles: string[];
  assignee_users: string[];
  status: ApprovalStatus;
  decided_by: string | null;
  decided_at: string | null;
  comment: string;
  expires_at: string | null;
  created_at: string;
}

export interface ApprovalListQuery {
  status?: ApprovalStatus;
  mine?: boolean;
  run_id?: string;
  limit?: number;
  cursor?: string;
}

export interface DecideApprovalRequest {
  decision: ApprovalDecision;
  comment?: string;
}

export interface ActionLogRecord {
  id: number;
  run_id: string | null;
  node_id: string | null;
  source_kind: ActionSourceKind;
  action_kind: ActionKind;
  target: string;
  payload: Record<string, unknown>;
  status: ActionLogStatus;
  idempotency_key: string;
  error_message: string | null;
  created_at: string;
}

export interface ActionLogListQuery {
  run_id?: string;
  status?: ActionLogStatus;
  limit?: number;
  cursor?: string;
}

export interface FlowHookSecretResponse {
  id: string;
  flow_id: string;
  slug: string;
  path: string;
  secret: string;
  created_at: string;
}

export interface JsonSchemaPropertySpec {
  type?: string | string[];
  title?: string;
  description?: string;
  default?: unknown;
  enum?: (string | number | boolean)[];
  minimum?: number;
  maximum?: number;
  items?: Record<string, unknown>;
  properties?: Record<string, unknown>;
}

export interface OperatorConfigSchema {
  type?: string;
  required?: string[];
  properties?: Record<string, JsonSchemaPropertySpec>;
  additionalProperties?: boolean;
}

export interface OperatorSpecDto {
  kind: FlowOperator;
  display_name: string;
  description: string;
  config_schema: OperatorConfigSchema;
}

// ============================================================================
// Canonical Fallback Operator Specs (9 Step 07 Operators, §07.4)
// ============================================================================

export const FALLBACK_OPERATOR_SPECS: readonly OperatorSpecDto[] = [
  {
    kind: "SOURCE_INGEST",
    display_name: "数据源同步 (SOURCE_INGEST)",
    description: "触发数据集入湖同步任务并可选等待完成",
    config_schema: {
      type: "object",
      required: ["dataset_id"],
      properties: {
        dataset_id: {
          type: "string",
          title: "数据集 UUID",
          description: "目标 AxHub 数据集 ID",
        },
        wait: {
          type: "boolean",
          title: "等待同步终态",
          default: true,
        },
        timeout_s: {
          type: "integer",
          title: "同步等待超时 (秒)",
          default: 300,
          minimum: 1,
          maximum: 86400,
        },
      },
    },
  },
  {
    kind: "SQL_TRANSFORM",
    display_name: "SQL 变换与校验 (SQL_TRANSFORM)",
    description: "执行受保护只读 OLAP SQL 或物化为派生视图数据集",
    config_schema: {
      type: "object",
      properties: {
        sql: {
          type: "string",
          title: "SQL 查询语句",
          description: "只读 SELECT 查询，支持 {{ ... }} 参数占位符",
        },
        saved_query_id: {
          type: "string",
          title: "保存查询 UUID (可选)",
          description: "与 sql 二选一",
        },
        materialize_to: {
          type: "string",
          title: "物化派生表名 (可选)",
          description: "填写后将结果写入 DERIVED_VIEW 数据集",
        },
        max_rows: {
          type: "integer",
          title: "最大返回行数",
          default: 200,
          minimum: 1,
          maximum: 1000,
        },
        params: {
          type: "object",
          title: "查询参数对象",
          default: {},
        },
      },
    },
  },
  {
    kind: "LLM_EXTRACT",
    display_name: "智能结构化抽取 (LLM_EXTRACT)",
    description: "调用模型按 JSON Schema 从文本中提取结构化字段",
    config_schema: {
      type: "object",
      required: ["input", "instruction", "output_schema"],
      properties: {
        input: {
          type: "string",
          title: "输入内容模板",
          description: "例如 {{ trigger.text }} 或上游节点输出",
        },
        instruction: {
          type: "string",
          title: "抽取指令",
          description: "指导模型提取目标实体与指标",
        },
        model: {
          type: "string",
          title: "模型标识 (可选)",
        },
        output_schema: {
          type: "object",
          title: "输出 JSON Schema",
          default: { type: "object", properties: {} },
        },
      },
    },
  },
  {
    kind: "XBLUE_MAP",
    display_name: "业务节点映射 (XBLUE_MAP)",
    description: "将湖表数据按 xBlueStudio 映射规则物化为图谱节点与关系",
    config_schema: {
      type: "object",
      required: ["mapping_id"],
      properties: {
        mapping_id: {
          type: "string",
          title: "映射规则 UUID",
        },
        mode: {
          type: "string",
          title: "物化模式",
          enum: ["INCREMENTAL", "FULL"],
          default: "INCREMENTAL",
        },
      },
    },
  },
  {
    kind: "AGG_TRIGGER",
    display_name: "图谱动态聚合 (AGG_TRIGGER)",
    description: "触发跨关系指标上卷聚合与派生指标重算",
    config_schema: {
      type: "object",
      properties: {
        type_name: {
          type: "string",
          title: "限定节点类型名 (留空重算全部)",
          description: "例如 Warehouse",
        },
      },
    },
  },
  {
    kind: "CAUSAL_SIM",
    display_name: "因果推演沙盘 (CAUSAL_SIM)",
    description: "在写时复制沙盘中执行多跳干预推演并评估影响",
    config_schema: {
      type: "object",
      required: ["actions"],
      properties: {
        sandbox_id: {
          type: "string",
          title: "沙盘 UUID",
        },
        compare_with: {
          type: "string",
          title: "对比场景 UUID (可选)",
        },
        actions: {
          type: "array",
          title: "干预动作列表 (JSON)",
          default: [],
        },
      },
    },
  },
  {
    kind: "HUMAN_APPROVAL",
    display_name: "人工审批门禁 (HUMAN_APPROVAL)",
    description: "挂起当前分支并生成审批单，待指派角色/用户决策后恢复",
    config_schema: {
      type: "object",
      required: ["title"],
      properties: {
        title: {
          type: "string",
          title: "审批标题",
          default: "流程关键动作审批",
        },
        detail_template: {
          type: "object",
          title: "审批上下文详情 (JSON)",
          default: {},
        },
        assignee_roles: {
          type: "array",
          title: "指派角色列表",
          default: ["ADMIN", "OPERATOR"],
        },
        assignee_users: {
          type: "array",
          title: "指派用户 UUID 列表",
          default: [],
        },
        expires_in_minutes: {
          type: "integer",
          title: "超时分钟数",
          default: 1440,
          minimum: 1,
        },
      },
    },
  },
  {
    kind: "ACTION_EMIT",
    display_name: "业务动作回写 (ACTION_EMIT)",
    description: "回写节点指标/属性或发送通知（TEST 环境自动记录为 DRY_RUN）",
    config_schema: {
      type: "object",
      required: ["actions"],
      properties: {
        dry_run: {
          type: "boolean",
          title: "强制演练模式 (DRY_RUN)",
          default: false,
        },
        actions: {
          type: "array",
          title: "动作列表 (JSON)",
          default: [
            {
              kind: "WRITE_NODE_METRIC",
              node_ref: "Warehouse:W-1",
              metric: "replenish_flag",
              value: 1,
            },
          ],
        },
      },
    },
  },
  {
    kind: "HTTP_CALL",
    display_name: "外部 HTTP 调用 (HTTP_CALL)",
    description: "经出站安全守卫校验后发送带幂等键的 HTTP 请求",
    config_schema: {
      type: "object",
      required: ["method", "url"],
      properties: {
        method: {
          type: "string",
          title: "HTTP 方法",
          enum: ["GET", "POST", "PUT", "PATCH", "DELETE"],
          default: "POST",
        },
        url: {
          type: "string",
          title: "正式环境 URL",
          description: "https://...",
        },
        test_url: {
          type: "string",
          title: "测试环境 URL (可选)",
          description: "TEST 环境优先调用，未填则仅记录不发送",
        },
        response_path: {
          type: "string",
          title: "响应提取路径 (可选)",
        },
        headers: {
          type: "object",
          title: "自定义请求头",
          default: {},
        },
        body: {
          type: "object",
          title: "请求体 JSON",
          default: {},
        },
      },
    },
  },
];

// ============================================================================
// Runtime Validation & Defensive Normalization (`zod` v4)
// ============================================================================

const CANONICAL_OPERATORS: readonly FlowOperator[] = [
  "SOURCE_INGEST",
  "SQL_TRANSFORM",
  "LLM_EXTRACT",
  "XBLUE_MAP",
  "AGG_TRIGGER",
  "CAUSAL_SIM",
  "HUMAN_APPROVAL",
  "ACTION_EMIT",
  "HTTP_CALL",
  "AGENT_TASK",
];

const CANONICAL_TRIGGERS: readonly TriggerType[] = [
  "MANUAL",
  "CRON",
  "OUTBOX_EVENT",
  "WEBHOOK",
  "API",
  "MCP",
  "RULE",
  "SIM_APPLY",
];

const CANONICAL_RUN_STATUSES: readonly RunStatus[] = [
  "PENDING",
  "RUNNING",
  "WAITING_APPROVAL",
  "SUCCEEDED",
  "FAILED",
  "CANCELED",
];

const CANONICAL_STEP_STATUSES: readonly StepStatus[] = [
  "PENDING",
  "RUNNING",
  "WAITING_APPROVAL",
  "SUCCEEDED",
  "FAILED",
  "SKIPPED",
];

function isPlainRecord(val: unknown): val is Record<string, unknown> {
  return typeof val === "object" && val !== null && !Array.isArray(val);
}

function toRecord(val: unknown): Record<string, unknown> {
  return isPlainRecord(val) ? val : {};
}

function normalizeOperator(raw: unknown): FlowOperator {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase();
    if (upper === "INGEST") return "SOURCE_INGEST";
    if (upper === "TRANSFORM") return "SQL_TRANSFORM";
    if (upper === "NODE_MAP") return "XBLUE_MAP";
    if (upper === "ACTION") return "ACTION_EMIT";
    if ((CANONICAL_OPERATORS as readonly string[]).includes(upper)) {
      return upper as FlowOperator;
    }
  }
  return "SQL_TRANSFORM";
}

function normalizeTriggerType(raw: unknown): TriggerType {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase();
    if ((CANONICAL_TRIGGERS as readonly string[]).includes(upper)) {
      return upper as TriggerType;
    }
  }
  return "MANUAL";
}

function normalizeEnvironment(raw: unknown): Environment {
  return raw === "PROD" ? "PROD" : "TEST";
}

function normalizeRunStatus(raw: unknown): RunStatus {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase();
    if ((CANONICAL_RUN_STATUSES as readonly string[]).includes(upper)) {
      return upper as RunStatus;
    }
  }
  return "PENDING";
}

function normalizeStepStatus(raw: unknown): StepStatus {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase();
    if ((CANONICAL_STEP_STATUSES as readonly string[]).includes(upper)) {
      return upper as StepStatus;
    }
  }
  return "PENDING";
}

export const DEFAULT_RETRY_POLICY: RetryPolicy = {
  max_attempts: 3,
  backoff_s: 10,
  backoff: "FIXED",
};

export const DEFAULT_TRIGGER_CFG: TriggerCfg = {
  type: "MANUAL",
};

const FlowPosSchema: z.ZodType<FlowPos> = z
  .unknown()
  .transform((raw): FlowPos => {
    const rec = toRecord(raw);
    return {
      x: typeof rec.x === "number" && Number.isFinite(rec.x) ? rec.x : 120,
      y: typeof rec.y === "number" && Number.isFinite(rec.y) ? rec.y : 120,
    };
  });

const RetryPolicySchema: z.ZodType<RetryPolicy> = z
  .unknown()
  .transform((raw): RetryPolicy => {
    const rec = toRecord(raw);
    const maxAttempts =
      typeof rec.max_attempts === "number" && Number.isFinite(rec.max_attempts)
        ? Math.min(10, Math.max(1, Math.trunc(rec.max_attempts)))
        : 3;
    const backoffS =
      typeof rec.backoff_s === "number" && Number.isFinite(rec.backoff_s)
        ? Math.min(3600, Math.max(1, Math.trunc(rec.backoff_s)))
        : 10;
    const backoff: BackoffStrategy =
      rec.backoff === "EXPONENTIAL" ? "EXPONENTIAL" : "FIXED";
    return {
      max_attempts: maxAttempts,
      backoff_s: backoffS,
      backoff,
    };
  });

export const FlowNodeDefSchema: z.ZodType<FlowNodeDef> = z
  .unknown()
  .transform((raw): FlowNodeDef => {
    const rec = toRecord(raw);
    const id = typeof rec.id === "string" && rec.id.trim() ? rec.id.trim() : "node_1";
    const kind = normalizeOperator(rec.kind);
    const label =
      typeof rec.label === "string" && rec.label.trim() ? rec.label : id;
    const timeoutS =
      typeof rec.timeout_s === "number" && Number.isFinite(rec.timeout_s)
        ? Math.min(86400, Math.max(1, Math.trunc(rec.timeout_s)))
        : 600;
    const onFailure: OnFailure =
      rec.on_failure === "CONTINUE" ? "CONTINUE" : "FAIL_RUN";
    const join: JoinMode =
      rec.join === "ALL_DONE"
        ? "ALL_DONE"
        : rec.join === "ANY_SUCCESS"
          ? "ANY_SUCCESS"
          : "ALL_SUCCESS";
    return {
      id,
      kind,
      label,
      config: toRecord(rec.config),
      retry: RetryPolicySchema.parse(rec.retry),
      timeout_s: timeoutS,
      on_failure: onFailure,
      join,
      pos: FlowPosSchema.parse(rec.pos),
    };
  });

export const FlowEdgeDefSchema: z.ZodType<FlowEdgeDef> = z
  .unknown()
  .transform((raw): FlowEdgeDef => {
    const rec = toRecord(raw);
    const whenVal =
      typeof rec.when === "string" && rec.when.trim().length > 0
        ? rec.when.trim()
        : null;
    return {
      from: typeof rec.from === "string" ? rec.from : "",
      to: typeof rec.to === "string" ? rec.to : "",
      when: whenVal,
    };
  });

export const TriggerCfgSchema: z.ZodType<TriggerCfg> = z
  .unknown()
  .transform((raw): TriggerCfg => {
    const rec = toRecord(raw);
    const kind = normalizeTriggerType(rec.type);
    const out: TriggerCfg = { type: kind };
    if (typeof rec.cron === "string") out.cron = rec.cron;
    if (typeof rec.timezone === "string") out.timezone = rec.timezone;
    if (typeof rec.catch_up === "boolean") out.catch_up = rec.catch_up;
    if (typeof rec.topic === "string") out.topic = rec.topic;
    if (typeof rec.filter === "string") out.filter = rec.filter;
    return out;
  });

export const FlowIssueSchema: z.ZodType<FlowIssue> = z
  .unknown()
  .transform((raw): FlowIssue => {
    const rec = toRecord(raw);
    return {
      code: typeof rec.code === "string" ? rec.code : "INVALID_ARGUMENT",
      message: typeof rec.message === "string" ? rec.message : "",
      severity: rec.severity === "WARNING" ? "WARNING" : "ERROR",
      node_id: typeof rec.node_id === "string" ? rec.node_id : null,
    };
  });

/** Converts legacy linear `steps` array into a sequential chain `(nodes, edges)` DAG per `07-STEP07-08` §07.1. */
export function convertLegacyStepsToGraph(rawSteps: unknown[]): FlowGraphDef {
  const nodes: FlowNodeDef[] = [];
  const edges: FlowEdgeDef[] = [];
  let prevId: string | null = null;

  rawSteps.forEach((item, idx) => {
    const rec = toRecord(item);
    const id =
      typeof rec.id === "string" && rec.id.trim().length > 0
        ? rec.id.trim()
        : `step_${idx + 1}`;
    const rawKind = typeof rec.kind === "string" ? rec.kind.trim().toUpperCase() : "";
    let kind: FlowOperator = "ACTION_EMIT";
    let config = { ...toRecord(rec.config) };
    if (rawKind === "INGEST") {
      kind = "SOURCE_INGEST";
    } else if (rawKind === "TRANSFORM") {
      kind = "SQL_TRANSFORM";
    } else if (rawKind === "NODE_MAP") {
      kind = "XBLUE_MAP";
    } else if (rawKind === "ACTION") {
      kind = "ACTION_EMIT";
    } else if ((CANONICAL_OPERATORS as readonly string[]).includes(rawKind)) {
      kind = rawKind as FlowOperator;
    } else {
      kind = "ACTION_EMIT";
      config = { ...config, dry_run: true };
    }

    const label =
      typeof rec.label === "string" && rec.label.trim().length > 0
        ? rec.label
        : id;

    nodes.push({
      id,
      kind,
      label,
      config,
      retry: { ...DEFAULT_RETRY_POLICY },
      timeout_s: 600,
      on_failure: "FAIL_RUN",
      join: "ALL_SUCCESS",
      pos: { x: 220 * idx + 60, y: 140 },
    });

    if (prevId !== null) {
      edges.push({ from: prevId, to: id, when: null });
    }
    prevId = id;
  });

  return { nodes, edges };
}

export const FlowRecordSchema: z.ZodType<FlowRecord> = z
  .unknown()
  .transform((raw): FlowRecord => {
    const rec = toRecord(raw);
    let nodes = Array.isArray(rec.nodes)
      ? rec.nodes.map((n) => FlowNodeDefSchema.parse(n))
      : [];
    let edges = Array.isArray(rec.edges)
      ? rec.edges
          .map((e) => FlowEdgeDefSchema.parse(e))
          .filter((e) => e.from.length > 0 && e.to.length > 0)
      : [];

    if (nodes.length === 0 && Array.isArray(rec.steps) && rec.steps.length > 0) {
      const converted = convertLegacyStepsToGraph(rec.steps);
      nodes = converted.nodes;
      edges = converted.edges;
    }

    return {
      id: typeof rec.id === "string" ? rec.id : "",
      name: typeof rec.name === "string" ? rec.name : "未命名流水线",
      description: typeof rec.description === "string" ? rec.description : "",
      slug: typeof rec.slug === "string" && rec.slug.length > 0 ? rec.slug : null,
      nodes,
      edges,
      trigger_cfg: TriggerCfgSchema.parse(rec.trigger_cfg),
      status: typeof rec.status === "string" ? rec.status : "DRAFT",
      created_by: typeof rec.created_by === "string" ? rec.created_by : null,
      last_run_at: typeof rec.last_run_at === "string" ? rec.last_run_at : null,
      created_at: typeof rec.created_at === "string" ? rec.created_at : "",
      updated_at: typeof rec.updated_at === "string" ? rec.updated_at : "",
      issues: Array.isArray(rec.issues)
        ? rec.issues.map((i) => FlowIssueSchema.parse(i))
        : [],
    };
  });

const ValidateFlowResponseSchema: z.ZodType<ValidateFlowResponse> = z
  .unknown()
  .transform((raw): ValidateFlowResponse => {
    const rec = toRecord(raw);
    const issues = Array.isArray(rec.issues)
      ? rec.issues.map((i) => FlowIssueSchema.parse(i))
      : [];
    const valid =
      typeof rec.valid === "boolean"
        ? rec.valid
        : issues.every((i) => i.severity !== "ERROR");
    return { valid, issues };
  });

const FlowVersionRecordSchema: z.ZodType<FlowVersionRecord> = z
  .unknown()
  .transform((raw): FlowVersionRecord => {
    const rec = toRecord(raw);
    return {
      id: typeof rec.id === "string" ? rec.id : "",
      flow_id: typeof rec.flow_id === "string" ? rec.flow_id : "",
      version: typeof rec.version === "number" ? rec.version : 1,
      nodes: Array.isArray(rec.nodes)
        ? rec.nodes.map((n) => FlowNodeDefSchema.parse(n))
        : [],
      edges: Array.isArray(rec.edges)
        ? rec.edges
            .map((e) => FlowEdgeDefSchema.parse(e))
            .filter((e) => e.from.length > 0 && e.to.length > 0)
        : [],
      trigger_cfg: TriggerCfgSchema.parse(rec.trigger_cfg),
      note: typeof rec.note === "string" ? rec.note : "",
      published_by:
        typeof rec.published_by === "string" ? rec.published_by : null,
      published_at:
        typeof rec.published_at === "string" ? rec.published_at : "",
    };
  });

const FlowDeploymentRecordSchema: z.ZodType<FlowDeploymentRecord> = z
  .unknown()
  .transform((raw): FlowDeploymentRecord => {
    const rec = toRecord(raw);
    return {
      flow_id: typeof rec.flow_id === "string" ? rec.flow_id : "",
      environment: normalizeEnvironment(rec.environment),
      version: typeof rec.version === "number" ? rec.version : 1,
      deployed_by: typeof rec.deployed_by === "string" ? rec.deployed_by : null,
      deployed_at: typeof rec.deployed_at === "string" ? rec.deployed_at : "",
    };
  });

const StartFlowRunResponseSchema: z.ZodType<StartFlowRunResponse> = z
  .unknown()
  .transform((raw): StartFlowRunResponse => {
    const rec = toRecord(raw);
    return {
      run_id:
        typeof rec.run_id === "string"
          ? rec.run_id
          : typeof rec.id === "string"
            ? rec.id
            : "",
      status: normalizeRunStatus(rec.status),
      existing: Boolean(rec.existing),
    };
  });

const FlowRunRecordSchema: z.ZodType<FlowRunRecord> = z
  .unknown()
  .transform((raw): FlowRunRecord => {
    const rec = toRecord(raw);
    return {
      id: typeof rec.id === "string" ? rec.id : "",
      flow_id: typeof rec.flow_id === "string" ? rec.flow_id : "",
      version: typeof rec.version === "number" ? rec.version : 1,
      environment: normalizeEnvironment(rec.environment),
      trigger_source: normalizeTriggerType(rec.trigger_source),
      trigger_payload: toRecord(rec.trigger_payload),
      idempotency_key:
        typeof rec.idempotency_key === "string" ? rec.idempotency_key : null,
      status: normalizeRunStatus(rec.status),
      outputs: toRecord(rec.outputs),
      error_message:
        typeof rec.error_message === "string" ? rec.error_message : null,
      started_by: typeof rec.started_by === "string" ? rec.started_by : null,
      created_at: typeof rec.created_at === "string" ? rec.created_at : "",
      started_at: typeof rec.started_at === "string" ? rec.started_at : null,
      finished_at: typeof rec.finished_at === "string" ? rec.finished_at : null,
    };
  });

const FlowStepRecordSchema: z.ZodType<FlowStepRecord> = z
  .unknown()
  .transform((raw): FlowStepRecord => {
    const rec = toRecord(raw);
    return {
      id: typeof rec.id === "number" ? rec.id : 0,
      run_id: typeof rec.run_id === "string" ? rec.run_id : "",
      node_id: typeof rec.node_id === "string" ? rec.node_id : "",
      attempt: typeof rec.attempt === "number" ? rec.attempt : 1,
      status: normalizeStepStatus(rec.status),
      job_id: typeof rec.job_id === "string" ? rec.job_id : null,
      input: toRecord(rec.input),
      output: rec.output !== undefined ? rec.output : null,
      output_ref: typeof rec.output_ref === "string" ? rec.output_ref : null,
      error_code: typeof rec.error_code === "string" ? rec.error_code : null,
      error_message:
        typeof rec.error_message === "string" ? rec.error_message : null,
      started_at: typeof rec.started_at === "string" ? rec.started_at : null,
      finished_at: typeof rec.finished_at === "string" ? rec.finished_at : null,
    };
  });

const ApprovalRecordSchema: z.ZodType<ApprovalRecord> = z
  .unknown()
  .transform((raw): ApprovalRecord => {
    const rec = toRecord(raw);
    const sourceKind: ApprovalSourceKind =
      rec.source_kind === "SIM_APPLY"
        ? "SIM_APPLY"
        : rec.source_kind === "AGENT"
          ? "AGENT"
          : "FLOW";
    const status: ApprovalStatus =
      rec.status === "APPROVED"
        ? "APPROVED"
        : rec.status === "REJECTED"
          ? "REJECTED"
          : rec.status === "EXPIRED"
            ? "EXPIRED"
            : "PENDING";
    return {
      id: typeof rec.id === "string" ? rec.id : "",
      run_id: typeof rec.run_id === "string" ? rec.run_id : null,
      node_id: typeof rec.node_id === "string" ? rec.node_id : null,
      source_kind: sourceKind,
      source_ref: typeof rec.source_ref === "string" ? rec.source_ref : "",
      title: typeof rec.title === "string" ? rec.title : "待审批项",
      detail: toRecord(rec.detail),
      assignee_roles: Array.isArray(rec.assignee_roles)
        ? rec.assignee_roles.filter((r): r is string => typeof r === "string")
        : [],
      assignee_users: Array.isArray(rec.assignee_users)
        ? rec.assignee_users.filter((u): u is string => typeof u === "string")
        : [],
      status,
      decided_by: typeof rec.decided_by === "string" ? rec.decided_by : null,
      decided_at: typeof rec.decided_at === "string" ? rec.decided_at : null,
      comment: typeof rec.comment === "string" ? rec.comment : "",
      expires_at: typeof rec.expires_at === "string" ? rec.expires_at : null,
      created_at: typeof rec.created_at === "string" ? rec.created_at : "",
    };
  });

const ActionLogRecordSchema: z.ZodType<ActionLogRecord> = z
  .unknown()
  .transform((raw): ActionLogRecord => {
    const rec = toRecord(raw);
    const sourceKind: ActionSourceKind =
      rec.source_kind === "SIM_APPLY" ||
      rec.source_kind === "AGENT" ||
      rec.source_kind === "RULE" ||
      rec.source_kind === "VIEW"
        ? rec.source_kind
        : "FLOW";
    const actionKind: ActionKind =
      rec.action_kind === "WRITE_NODE_PROPERTY" ||
      rec.action_kind === "HTTP" ||
      rec.action_kind === "NOTIFY" ||
      rec.action_kind === "WORK_ITEM"
        ? rec.action_kind
        : "WRITE_NODE_METRIC";
    const status: ActionLogStatus =
      rec.status === "DRY_RUN"
        ? "DRY_RUN"
        : rec.status === "FAILED"
          ? "FAILED"
          : "DONE";
    return {
      id: typeof rec.id === "number" ? rec.id : 0,
      run_id: typeof rec.run_id === "string" ? rec.run_id : null,
      node_id: typeof rec.node_id === "string" ? rec.node_id : null,
      source_kind: sourceKind,
      action_kind: actionKind,
      target: typeof rec.target === "string" ? rec.target : "",
      payload: toRecord(rec.payload),
      status,
      idempotency_key:
        typeof rec.idempotency_key === "string" ? rec.idempotency_key : "",
      error_message:
        typeof rec.error_message === "string" ? rec.error_message : null,
      created_at: typeof rec.created_at === "string" ? rec.created_at : "",
    };
  });

const OperatorSpecDtoSchema: z.ZodType<OperatorSpecDto> = z
  .unknown()
  .transform((raw): OperatorSpecDto => {
    const rec = toRecord(raw);
    const kind = normalizeOperator(rec.kind);
    const schemaRec = toRecord(rec.config_schema);
    const propsRec = toRecord(schemaRec.properties);
    const properties: Record<string, JsonSchemaPropertySpec> = {};
    for (const [k, v] of Object.entries(propsRec)) {
      const p = toRecord(v);
      const spec: JsonSchemaPropertySpec = {};
      if (typeof p.type === "string" || Array.isArray(p.type)) {
        spec.type = p.type as string | string[];
      }
      if (typeof p.title === "string") spec.title = p.title;
      if (typeof p.description === "string") spec.description = p.description;
      if (p.default !== undefined) spec.default = p.default;
      if (Array.isArray(p.enum)) {
        spec.enum = p.enum.filter(
          (item): item is string | number | boolean =>
            typeof item === "string" ||
            typeof item === "number" ||
            typeof item === "boolean"
        );
      }
      if (typeof p.minimum === "number") spec.minimum = p.minimum;
      if (typeof p.maximum === "number") spec.maximum = p.maximum;
      if (isPlainRecord(p.items)) spec.items = p.items;
      if (isPlainRecord(p.properties)) spec.properties = p.properties;
      properties[k] = spec;
    }

    return {
      kind,
      display_name:
        typeof rec.display_name === "string" && rec.display_name.trim()
          ? rec.display_name
          : kind,
      description:
        typeof rec.description === "string" ? rec.description : "",
      config_schema: {
        type: typeof schemaRec.type === "string" ? schemaRec.type : "object",
        required: Array.isArray(schemaRec.required)
          ? schemaRec.required.filter((r): r is string => typeof r === "string")
          : [],
        properties,
        additionalProperties:
          typeof schemaRec.additionalProperties === "boolean"
            ? schemaRec.additionalProperties
            : undefined,
      },
    };
  });

function parsePaginatedEnvelope<T>(
  itemSchema: z.ZodType<T>,
  raw: unknown
): FlowPaginatedList<T> {
  if (Array.isArray(raw)) {
    return {
      items: raw.map((entry) => itemSchema.parse(entry)),
      next_cursor: null,
    };
  }
  const rec = toRecord(raw);
  const rawItems = Array.isArray(rec.items) ? rec.items : [];
  return {
    items: rawItems.map((entry) => itemSchema.parse(entry)),
    next_cursor: typeof rec.next_cursor === "string" ? rec.next_cursor : null,
  };
}

function buildQuery(
  params: Record<string, string | number | boolean | undefined | null>
): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (k === "tenant_id") continue;
    if (v !== undefined && v !== null && String(v).length > 0) {
      sp.set(k, String(v));
    }
  }
  const qs = sp.toString();
  return qs ? `?${qs}` : "";
}

function sanitizeNodeForWire(node: FlowNodeDef): FlowNodeDef {
  return {
    id: node.id.trim(),
    kind: node.kind,
    label: node.label,
    config: node.config,
    retry: {
      max_attempts: Math.min(10, Math.max(1, Math.trunc(node.retry.max_attempts))),
      backoff_s: Math.min(3600, Math.max(1, Math.trunc(node.retry.backoff_s))),
      backoff: node.retry.backoff,
    },
    timeout_s: Math.min(86400, Math.max(1, Math.trunc(node.timeout_s))),
    on_failure: node.on_failure,
    join: node.join,
    pos: {
      x: Number.isFinite(node.pos.x) ? Math.round(node.pos.x) : 120,
      y: Number.isFinite(node.pos.y) ? Math.round(node.pos.y) : 120,
    },
  };
}

function sanitizeEdgeForWire(
  edge: FlowEdgeDef
): { from: string; to: string; when?: string } {
  const trimmedWhen = edge.when?.trim() ?? "";
  if (trimmedWhen.length > 0) {
    return {
      from: edge.from.trim(),
      to: edge.to.trim(),
      when: trimmedWhen,
    };
  }
  return {
    from: edge.from.trim(),
    to: edge.to.trim(),
  };
}

function sanitizeTriggerForWire(
  cfg: TriggerCfg
): Record<string, string | boolean> {
  const wire: Record<string, string | boolean> = { type: cfg.type };
  if (cfg.type === "CRON") {
    if (cfg.cron && cfg.cron.trim().length > 0) {
      wire.cron = cfg.cron.trim();
    }
    if (cfg.timezone && cfg.timezone.trim().length > 0) {
      wire.timezone = cfg.timezone.trim();
    }
    wire.catch_up = Boolean(cfg.catch_up);
  } else if (cfg.type === "OUTBOX_EVENT") {
    if (cfg.topic && cfg.topic.trim().length > 0) {
      wire.topic = cfg.topic.trim();
    }
    if (cfg.filter && cfg.filter.trim().length > 0) {
      wire.filter = cfg.filter.trim();
    }
  }
  return wire;
}

/** Computes strict DAG ancestors of `targetNodeId` via reverse BFS over `edges`. */
export function computeDagAncestors(
  targetNodeId: string,
  nodes: readonly FlowNodeDef[],
  edges: readonly FlowEdgeDef[]
): FlowNodeDef[] {
  const incoming = new Map<string, string[]>();
  for (const edge of edges) {
    const list = incoming.get(edge.to) ?? [];
    list.push(edge.from);
    incoming.set(edge.to, list);
  }

  const visited = new Set<string>();
  const queue: string[] = [...(incoming.get(targetNodeId) ?? [])];
  while (queue.length > 0) {
    const curr = queue.shift();
    if (!curr || curr === targetNodeId || visited.has(curr)) continue;
    visited.add(curr);
    for (const pred of incoming.get(curr) ?? []) {
      if (!visited.has(pred) && pred !== targetNodeId) {
        queue.push(pred);
      }
    }
  }

  return nodes.filter((n) => visited.has(n.id));
}

// ============================================================================
// AxFlow API Client (`/api/axflow/*`)
// ============================================================================

export const axflowApi = {
  listOperators: async (): Promise<OperatorSpecDto[]> => {
    const raw = await requestJson<unknown>("/api/axflow/operators");
    if (Array.isArray(raw)) {
      const parsed = raw.map((item) => OperatorSpecDtoSchema.parse(item));
      return parsed.length > 0 ? parsed : [...FALLBACK_OPERATOR_SPECS];
    }
    const rec = toRecord(raw);
    if (Array.isArray(rec.items)) {
      const parsed = rec.items.map((item) => OperatorSpecDtoSchema.parse(item));
      return parsed.length > 0 ? parsed : [...FALLBACK_OPERATOR_SPECS];
    }
    return [...FALLBACK_OPERATOR_SPECS];
  },

  listFlows: async (
    query?: FlowListQuery
  ): Promise<FlowPaginatedList<FlowRecord>> => {
    const qs = buildQuery({
      q: query?.q,
      limit: query?.limit,
      cursor: query?.cursor,
    });
    const raw = await requestJson<unknown>(`/api/axflow/flows${qs}`);
    return parsePaginatedEnvelope(FlowRecordSchema, raw);
  },

  getFlow: async (id: string): Promise<FlowRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}`
    );
    return FlowRecordSchema.parse(raw);
  },

  createFlow: async (req: CreateFlowRequest): Promise<FlowRecord> => {
    const body: Record<string, unknown> = {
      name: req.name.trim(),
      description: req.description?.trim() ?? "",
      nodes: (req.nodes ?? []).map(sanitizeNodeForWire),
      edges: (req.edges ?? []).map(sanitizeEdgeForWire),
      trigger_cfg: sanitizeTriggerForWire(req.trigger_cfg ?? DEFAULT_TRIGGER_CFG),
    };
    if (req.slug !== undefined && req.slug.trim().length > 0) {
      body.slug = req.slug.trim();
    }
    const raw = await requestJson<unknown>("/api/axflow/flows", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return FlowRecordSchema.parse(raw);
  },

  updateFlow: async (
    id: string,
    req: UpdateFlowRequest
  ): Promise<FlowRecord> => {
    const body: Record<string, unknown> = {};
    if (req.name !== undefined) body.name = req.name.trim();
    if (req.description !== undefined) body.description = req.description;
    if (req.slug !== undefined) body.slug = req.slug.trim();
    if (req.nodes !== undefined) {
      body.nodes = req.nodes.map(sanitizeNodeForWire);
    }
    if (req.edges !== undefined) {
      body.edges = req.edges.map(sanitizeEdgeForWire);
    }
    if (req.trigger_cfg !== undefined) {
      body.trigger_cfg = sanitizeTriggerForWire(req.trigger_cfg);
    }
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return FlowRecordSchema.parse(raw);
  },

  deleteFlow: async (id: string): Promise<{ deleted: boolean }> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    );
    const rec = toRecord(raw);
    return {
      deleted: typeof rec.deleted === "boolean" ? rec.deleted : true,
    };
  },

  validateFlow: async (id: string): Promise<ValidateFlowResponse> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}/validate`,
      { method: "POST" }
    );
    return ValidateFlowResponseSchema.parse(raw);
  },

  listVersions: async (id: string): Promise<FlowVersionsListResponse> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}/versions`
    );
    if (Array.isArray(raw)) {
      return {
        items: raw.map((v) => FlowVersionRecordSchema.parse(v)),
        deployments: [],
        next_cursor: null,
      };
    }
    const rec = toRecord(raw);
    const items = Array.isArray(rec.items)
      ? rec.items.map((v) => FlowVersionRecordSchema.parse(v))
      : [];
    const deployments = Array.isArray(rec.deployments)
      ? rec.deployments.map((d) => FlowDeploymentRecordSchema.parse(d))
      : [];
    return {
      items,
      deployments,
      next_cursor: typeof rec.next_cursor === "string" ? rec.next_cursor : null,
    };
  },

  publishVersion: async (
    id: string,
    req?: PublishFlowVersionRequest
  ): Promise<FlowVersionRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}/versions`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ note: req?.note ?? "" }),
      }
    );
    return FlowVersionRecordSchema.parse(raw);
  },

  deployFlow: async (
    id: string,
    req: DeployFlowRequest
  ): Promise<FlowDeploymentRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(id)}/deploy`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          environment: req.environment,
          version: req.version,
        }),
      }
    );
    return FlowDeploymentRecordSchema.parse(raw);
  },

  startRun: async (
    flowId: string,
    req?: StartFlowRunRequest
  ): Promise<StartFlowRunResponse> => {
    const body: Record<string, unknown> = {};
    if (req?.environment !== undefined) body.environment = req.environment;
    if (req?.version !== undefined) body.version = req.version;
    if (req?.payload !== undefined) body.payload = req.payload;
    if (req?.idempotency_key && req.idempotency_key.trim().length > 0) {
      body.idempotency_key = req.idempotency_key.trim();
    }
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(flowId)}/runs`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return StartFlowRunResponseSchema.parse(raw);
  },

  listRuns: async (
    flowId: string,
    query?: FlowRunListQuery
  ): Promise<FlowPaginatedList<FlowRunRecord>> => {
    const qs = buildQuery({
      environment: query?.environment,
      status: query?.status,
      limit: query?.limit,
      cursor: query?.cursor,
    });
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(flowId)}/runs${qs}`
    );
    return parsePaginatedEnvelope(FlowRunRecordSchema, raw);
  },

  getRun: async (runId: string): Promise<FlowRunRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/runs/${encodeURIComponent(runId)}`
    );
    return FlowRunRecordSchema.parse(raw);
  },

  listRunSteps: async (
    runId: string
  ): Promise<FlowPaginatedList<FlowStepRecord>> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/runs/${encodeURIComponent(runId)}/steps`
    );
    return parsePaginatedEnvelope(FlowStepRecordSchema, raw);
  },

  cancelRun: async (runId: string): Promise<FlowRunRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/runs/${encodeURIComponent(runId)}/cancel`,
      { method: "POST" }
    );
    return FlowRunRecordSchema.parse(raw);
  },

  retryRun: async (runId: string): Promise<FlowRunRecord> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/runs/${encodeURIComponent(runId)}/retry`,
      { method: "POST" }
    );
    return FlowRunRecordSchema.parse(raw);
  },

  rotateWebhookSecret: async (
    flowId: string
  ): Promise<FlowHookSecretResponse> => {
    const raw = await requestJson<unknown>(
      `/api/axflow/flows/${encodeURIComponent(flowId)}/hook`,
      { method: "POST" }
    );
    const rec = toRecord(raw);
    return {
      id: typeof rec.id === "string" ? rec.id : "",
      flow_id: typeof rec.flow_id === "string" ? rec.flow_id : flowId,
      slug: typeof rec.slug === "string" ? rec.slug : "",
      path: typeof rec.path === "string" ? rec.path : "",
      secret: typeof rec.secret === "string" ? rec.secret : "",
      created_at: typeof rec.created_at === "string" ? rec.created_at : "",
    };
  },

  listApprovals: async (
    query?: ApprovalListQuery
  ): Promise<FlowPaginatedList<ApprovalRecord>> => {
    const qs = buildQuery({
      status: query?.status,
      mine: query?.mine,
      run_id: query?.run_id,
      limit: query?.limit,
      cursor: query?.cursor,
    });
    const raw = await requestJson<unknown>(`/api/axflow/approvals${qs}`);
    return parsePaginatedEnvelope(ApprovalRecordSchema, raw);
  },

  decideApproval: async (
    approvalId: string,
    req: DecideApprovalRequest
  ): Promise<ApprovalRecord> => {
    const body: { decision: ApprovalDecision; comment?: string } = {
      decision: req.decision,
    };
    if (req.comment !== undefined) {
      body.comment = req.comment;
    }
    const raw = await requestJson<unknown>(
      `/api/axflow/approvals/${encodeURIComponent(approvalId)}/decide`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return ApprovalRecordSchema.parse(raw);
  },

  listActions: async (
    query?: ActionLogListQuery
  ): Promise<FlowPaginatedList<ActionLogRecord>> => {
    const qs = buildQuery({
      run_id: query?.run_id,
      status: query?.status,
      limit: query?.limit,
      cursor: query?.cursor,
    });
    const raw = await requestJson<unknown>(`/api/axflow/actions${qs}`);
    return parsePaginatedEnvelope(ActionLogRecordSchema, raw);
  },
};
