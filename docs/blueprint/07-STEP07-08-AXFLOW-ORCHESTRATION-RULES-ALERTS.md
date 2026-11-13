# 07 · Step 07：AxFlow DAG 引擎、持久化执行与流程版本 ／ Step 08：规则引擎、告警中心与通知渠道

AxFlow 把数据接入、变换、抽取、映射、聚合、推演、审批、动作与智能体任务编排为有向无环图（DAG）。每个节点作为一个持久化任务执行，状态全部落库，进程重启或 Worker 崩溃后可从断点继续。规则引擎持续判定指标与数据质量，产生告警、发送通知，并可触发流程。

---

# Step 07：AxFlow DAG 引擎、持久化执行与流程版本

## 07.0 目标、前置与契约

- **目标**：DAG 定义与校验；10 种算子；8 种触发器；节点级重试、超时、失败策略、条件分支、汇合语义；人工审批暂停与恢复；测试 / 正式两套部署与流程版本；运行历史与逐节点日志；流程画布。
- **前置**：Step 06 通过。
- **契约清单**：`0007_axflow_orchestration.sql`；`axpoint-flow` 公开类型（`FlowGraph`、`FlowNode`、`FlowEdge`、`TriggerCfg`、`RunRecord`、`StepRecord`）与对象安全 trait `Operator`；`axpoint-security::net_guard`；`docs/API_CONTRACT.md` 的 `/api/axflow/*`；`lib/api/axflow.ts`。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0007_axflow_orchestration.sql` | §07.1 |
| 新建 | `axpoint-core/crates/axpoint-flow/{Cargo.toml, src/lib.rs}` | Crate 入口 |
| 新建 | `axpoint-core/crates/axpoint-flow/src/{graph.rs, validate.rs, template.rs, engine.rs, scheduler.rs, triggers.rs}` | DAG、校验、参数模板、调度推进、Cron 与事件触发 |
| 新建 | `axpoint-core/crates/axpoint-flow/src/operators/{mod.rs, source_ingest.rs, sql_transform.rs, llm_extract.rs, xblue_map.rs, agg_trigger.rs, causal_sim.rs, human_approval.rs, action_emit.rs, http_call.rs}` | 算子（`AGENT_TASK` 在 Step 09 注册） |
| 新建 | `axpoint-core/crates/axpoint-security/src/net_guard.rs` | Rust 侧出站地址校验（规则同 Step 01 §01.1） |
| 新建 | `axpoint-core/crates/axpoint-db/src/flow_repo.rs` | 流程、版本、部署、运行、步骤日志、审批、动作日志 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/topology.rs` | `AxFlowDef` 增加 `nodes`、`edges`、`trigger_cfg`；`CreateAxFlowRequest` 接受 `nodes/edges` 或仓库已有的 `steps`（`steps` 按 §07.1 规则转换为链式 DAG） |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/axflow.rs` | §07.8 路由 |
| 修改 | `axpoint-core/crates/axpoint-server/src/background.rs` | 注册 `FLOW_STEP` 处理器、`approval.decided` 与 `OUTBOX_EVENT` 触发订阅者、流程 Cron 扫描 |
| 修改 | `xbluenode-web/src/pages/AxFlowPage.tsx`；新建 `src/components/axflow/{FlowCanvas.tsx, OperatorPalette.tsx, NodeConfigForm.tsx, TriggerEditor.tsx, RunHistory.tsx, RunTimeline.tsx, ApprovalInbox.tsx, VersionDeployPanel.tsx}` | 流程工作台 |
| 新建 | `scripts/seed/07-axflow.sh` | 演示流程（同步 → 映射 → 聚合 → 审批 → 动作） |
| 新建 | `scripts/gate/step07.sh` | 门禁 |

---

## 07.1 迁移 `deploy/migrations/0007_axflow_orchestration.sql`

```sql
-- 0007_axflow_orchestration.sql：DAG 定义、版本、部署、运行、步骤日志、审批、动作日志、流程 Webhook

ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS nodes       JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS edges       JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS trigger_cfg JSONB NOT NULL DEFAULT '{"type":"MANUAL"}'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS slug        TEXT;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS created_by  UUID;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW();
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_flows_name ON ax_flows (tenant_id, name);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_flows_slug ON ax_flows (tenant_id, slug) WHERE slug IS NOT NULL;

-- 仓库已有的线性 steps 转换为链式 DAG：
--   kind 映射 INGEST→SOURCE_INGEST、TRANSFORM→SQL_TRANSFORM、NODE_MAP→XBLUE_MAP、ACTION→ACTION_EMIT，其他值→ACTION_EMIT（dry_run=true）
--   节点 id 沿用 step.id，边按数组顺序连接
UPDATE ax_flows f
   SET nodes = sub.nodes, edges = sub.edges
  FROM (
    SELECT id,
           jsonb_agg(jsonb_build_object(
               'id', s->>'id',
               'kind', CASE s->>'kind' WHEN 'INGEST' THEN 'SOURCE_INGEST' WHEN 'TRANSFORM' THEN 'SQL_TRANSFORM'
                                       WHEN 'NODE_MAP' THEN 'XBLUE_MAP' ELSE 'ACTION_EMIT' END,
               'label', s->>'label',
               'config', CASE WHEN s->>'kind' IN ('INGEST','TRANSFORM','NODE_MAP','ACTION') THEN s->'config'
                              ELSE (s->'config') || '{"dry_run":true}'::jsonb END,
               'pos', jsonb_build_object('x', 200 * (ord - 1), 'y', 120)) ORDER BY ord) AS nodes,
           COALESCE(jsonb_agg(jsonb_build_object('from', prev_id, 'to', s->>'id')) FILTER (WHERE prev_id IS NOT NULL), '[]'::jsonb) AS edges
      FROM (
        SELECT f2.id, s, ord, lag(s->>'id') OVER (PARTITION BY f2.id ORDER BY ord) AS prev_id
          FROM ax_flows f2, jsonb_array_elements(f2.steps) WITH ORDINALITY AS t(s, ord)
      ) x
     GROUP BY id
  ) sub
 WHERE f.id = sub.id AND f.nodes = '[]'::jsonb AND jsonb_array_length(f.steps) > 0;

CREATE TABLE IF NOT EXISTS ax_flow_versions (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    flow_id      UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    version      INT NOT NULL,
    nodes        JSONB NOT NULL,
    edges        JSONB NOT NULL,
    trigger_cfg  JSONB NOT NULL,
    note         TEXT NOT NULL DEFAULT '',
    published_by UUID,
    published_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (flow_id, version)
);

CREATE TABLE IF NOT EXISTS ax_flow_deployments (
    tenant_id   TEXT NOT NULL,
    flow_id     UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    environment TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    version     INT NOT NULL,
    deployed_by UUID,
    deployed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (flow_id, environment)
);

CREATE TABLE IF NOT EXISTS ax_flow_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    flow_id         UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    version         INT NOT NULL,
    environment     TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    trigger_source  TEXT NOT NULL CHECK (trigger_source IN ('MANUAL','CRON','OUTBOX_EVENT','WEBHOOK','API','MCP','RULE','SIM_APPLY')),
    trigger_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    idempotency_key TEXT,
    status          TEXT NOT NULL DEFAULT 'PENDING'
                    CHECK (status IN ('PENDING','RUNNING','WAITING_APPROVAL','SUCCEEDED','FAILED','CANCELED')),
    outputs         JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_message   TEXT,
    started_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at      TIMESTAMPTZ,
    finished_at     TIMESTAMPTZ,
    UNIQUE (flow_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_ax_flow_runs_flow ON ax_flow_runs (tenant_id, flow_id, created_at DESC);

CREATE TABLE IF NOT EXISTS ax_flow_step_logs (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    run_id      UUID NOT NULL REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL,
    attempt     INT NOT NULL DEFAULT 1,
    status      TEXT NOT NULL CHECK (status IN ('PENDING','RUNNING','WAITING_APPROVAL','SUCCEEDED','FAILED','SKIPPED')),
    job_id      UUID,
    input       JSONB NOT NULL DEFAULT '{}'::jsonb,
    output      JSONB,
    output_ref  TEXT,                         -- 输出超过 1 MB 时写文件，相对 AXPOINT_DATA_ROOT
    error_code  TEXT,
    error_message TEXT,
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    UNIQUE (run_id, node_id, attempt)
);

CREATE TABLE IF NOT EXISTS ax_approvals (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    run_id          UUID REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    node_id         TEXT,
    source_kind     TEXT NOT NULL DEFAULT 'FLOW' CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT')),
    source_ref      TEXT NOT NULL DEFAULT '',
    title           TEXT NOT NULL,
    detail          JSONB NOT NULL DEFAULT '{}'::jsonb,
    assignee_roles  TEXT[] NOT NULL DEFAULT '{}',
    assignee_users  UUID[] NOT NULL DEFAULT '{}',
    status          TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','APPROVED','REJECTED','EXPIRED')),
    decided_by      UUID,
    decided_at      TIMESTAMPTZ,
    comment         TEXT NOT NULL DEFAULT '',
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_approvals_pending ON ax_approvals (tenant_id, status) WHERE status = 'PENDING';

CREATE TABLE IF NOT EXISTS ax_action_log (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    run_id      UUID,
    node_id     TEXT,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT','RULE')),
    action_kind TEXT NOT NULL CHECK (action_kind IN ('WRITE_NODE_METRIC','WRITE_NODE_PROPERTY','HTTP','NOTIFY','WORK_ITEM')),
    target      TEXT NOT NULL,
    payload     JSONB NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('DONE','DRY_RUN','FAILED')),
    idempotency_key TEXT NOT NULL,
    error_message TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS ax_flow_hooks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    flow_id       UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    credential_id UUID NOT NULL REFERENCES ds_credentials(id),   -- Bearer 秘密，密钥环加密
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (flow_id)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_flow_versions','ax_flow_deployments','ax_flow_runs','ax_flow_step_logs','ax_approvals','ax_action_log','ax_flow_hooks'
]::regclass[]) AS t;

CREATE OR REPLACE FUNCTION ax_scheduled_flows()
RETURNS TABLE (tenant_id TEXT, flow_id UUID, version INT, trigger_cfg JSONB, last_run_at TIMESTAMPTZ)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.flow_id, d.version, v.trigger_cfg,
           (SELECT max(r.created_at) FROM ax_flow_runs r WHERE r.flow_id = d.flow_id AND r.trigger_source = 'CRON')
      FROM ax_flow_deployments d
      JOIN ax_flow_versions v ON v.flow_id = d.flow_id AND v.version = d.version
     WHERE d.environment = 'PROD' AND v.trigger_cfg->>'type' = 'CRON';
$$;

CREATE OR REPLACE FUNCTION ax_event_flows(p_topic TEXT)
RETURNS TABLE (tenant_id TEXT, flow_id UUID, version INT, trigger_cfg JSONB)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.flow_id, d.version, v.trigger_cfg
      FROM ax_flow_deployments d
      JOIN ax_flow_versions v ON v.flow_id = d.flow_id AND v.version = d.version
     WHERE d.environment = 'PROD' AND v.trigger_cfg->>'type' = 'OUTBOX_EVENT' AND v.trigger_cfg->>'topic' = p_topic;
$$;

CREATE OR REPLACE FUNCTION ax_resolve_flow_hook(p_tenant TEXT, p_slug TEXT)
RETURNS TABLE (flow_id UUID, credential_id UUID)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT f.id, h.credential_id
      FROM ax_flows f JOIN ax_flow_hooks h ON h.flow_id = f.id
     WHERE f.tenant_id = p_tenant AND f.slug = p_slug AND h.enabled;
$$;

REVOKE ALL ON FUNCTION ax_scheduled_flows() FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_event_flows(TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_resolve_flow_hook(TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_scheduled_flows() TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_event_flows(TEXT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_resolve_flow_hook(TEXT, TEXT) TO axpoint_app;
```

`steps` 列保留为只读的历史字段：新接口不再写入，`AxFlowDef` 序列化时不再输出。

---

## 07.2 DAG 定义与校验

```rust
pub struct FlowGraph { pub nodes: Vec<FlowNode>, pub edges: Vec<FlowEdge> }

pub struct FlowNode {
    pub id: String,                       // ^[a-z][a-z0-9_]{0,31}$，流程内唯一
    pub kind: FlowOperator,               // A1 FlowOperator
    pub label: String,
    pub config: serde_json::Value,        // 算子配置，结构见 §07.4
    pub retry: RetryPolicy,               // {max_attempts: 1..=10（默认 3）, backoff_s: 1..=3600（默认 10）, backoff: FIXED|EXPONENTIAL}
    pub timeout_s: u32,                   // 1..=86_400，默认 600
    pub on_failure: OnFailure,            // FAIL_RUN（默认）| CONTINUE
    pub join: JoinMode,                   // ALL_SUCCESS（默认）| ALL_DONE | ANY_SUCCESS
    pub pos: Pos,
}

pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub when: Option<String>,             // Step 06 表达式，变量为上游输出的顶层字段
}
```

校验（`validate.rs`，发布与保存时执行）：

| # | 规则 | code |
| :--- | :--- | :--- |
| 1 | 节点 id 格式与唯一；节点数 ≤ 200，边数 ≤ 1 000 | `INVALID_ARGUMENT` |
| 2 | 边端点存在；无自环；无重复边 | `INVALID_ARGUMENT` |
| 3 | 无环（Kahn 拓扑排序，剩余节点即环上节点，错误信息列出环路） | `CYCLE_DETECTED` |
| 4 | 至少一个根节点 | `INVALID_ARGUMENT` |
| 5 | 算子已注册（`AGENT_TASK` 在 Step 09 之前未注册） | `UNSUPPORTED_FORMAT` |
| 6 | `config` 通过该算子的 JSON Schema（`jsonschema` crate） | `SCHEMA_INVALID` |
| 7 | `when` 表达式可解析；参数模板引用的节点必须是当前节点的祖先 | `EXPR_INVALID` |
| 8 | 触发器配置合法（Cron 可解析、事件主题属于 `OutboxTopic`） | `INVALID_ARGUMENT` |

### 07.2.1 参数模板（`template.rs`）

算子 `config` 中的字符串可包含 `{{ path }}` 占位符，只做取值替换，不执行代码：

| 路径前缀 | 值 |
| :--- | :--- |
| `trigger.<json path>` | 触发载荷 |
| `nodes.<node_id>.output.<json path>` | 祖先节点输出 |
| `run.id`、`run.environment`、`run.started_at` | 运行信息 |
| `flow.id`、`flow.version` | 流程信息 |

整个字符串恰好是一个占位符时保留原始 JSON 类型；否则转为字符串拼接。路径不存在时节点失败 `TEMPLATE_UNRESOLVED`。

---

## 07.3 持久化执行引擎（`engine.rs`）

### 07.3.1 状态机

```text
Run:  PENDING → RUNNING → (WAITING_APPROVAL ⇄ RUNNING) → SUCCEEDED | FAILED | CANCELED
Step: PENDING → RUNNING → SUCCEEDED | FAILED | SKIPPED | WAITING_APPROVAL → SUCCEEDED | FAILED
```

### 07.3.2 推进算法

1. **创建运行**（`start_run`）：解析目标版本（`environment` 的部署版本，或显式 `version`），写 `ax_flow_runs(PENDING)`；同一 `(flow_id, idempotency_key)` 已存在时返回已有运行。为所有根节点写 `ax_flow_step_logs(PENDING)` 并入队 `FLOW_STEP {run_id, node_id, attempt}`（幂等键 `<run_id>:<node_id>:<attempt>`），运行置 `RUNNING`。
2. **执行节点**（`FLOW_STEP` 处理器）：
   - 读取运行与版本定义，渲染参数模板，写步骤 `RUNNING`；
   - 调用算子 `execute(ctx)`，以 `tokio::time::timeout(timeout_s)` 包裹；`ctx` 提供 `idempotency_key = "<run_id>:<node_id>"`、租户主体（运行发起者或触发器对应的服务主体）、环境、日志写入器；
   - 成功：写输出（> 1 MB 写文件并记录 `output_ref`）与 `SUCCEEDED`；
   - 失败：`attempt < max_attempts` 时写当前尝试 `FAILED` 并以退避时间入队下一次尝试（`next_run_at`）；否则最终 `FAILED`；
   - 算子返回 `Suspend(approval_id)` 时步骤置 `WAITING_APPROVAL`，运行置 `WAITING_APPROVAL`（若无其他节点在执行）。
3. **推进后继**（与第 2 步结果写入同一事务）：对每个后继节点，当其全部前驱均已终态（`SUCCEEDED / FAILED / SKIPPED`）时：
   - 计算入边是否“激活”：前驱 `SUCCEEDED` 且 `when` 为真（或无 `when`）；
   - `ALL_SUCCESS`：全部入边激活 → 入队；否则 `SKIPPED`；
   - `ALL_DONE`：总是入队；
   - `ANY_SUCCESS`：至少一条激活 → 入队；否则 `SKIPPED`；
   - 使用 `SELECT ... FOR UPDATE` 锁住运行行，保证并发完成的两个前驱只会让后继入队一次（后继 `PENDING` 行的唯一约束兜底）。
4. **结束判定**：运行中不存在 `PENDING / RUNNING / WAITING_APPROVAL` 的步骤时结束：存在 `on_failure=FAIL_RUN` 的最终失败步骤 → `FAILED`；否则 `SUCCEEDED`。`outputs` 为所有叶子节点输出的映射。`on_failure=FAIL_RUN` 的节点最终失败时，立即把运行置 `FAILED`，其余 `PENDING` 步骤置 `SKIPPED`，正在执行的步骤完成后不再推进。
5. **取消**：`POST /runs/{id}/cancel` 置 `CANCELED`，`PENDING` 步骤置 `SKIPPED` 并把对应任务置 `CANCELED`；正在执行的算子在下一个检查点（算子内部轮询 `ctx.is_canceled()`）退出。
6. **崩溃恢复**：Worker 崩溃后任务租约过期，`ax_lease_jobs` 重新派发；算子必须对 `idempotency_key` 幂等（见各算子“幂等”栏）。

### 07.3.3 审批恢复

`approval.decided` 外发事件的订阅者：`APPROVED` → 步骤 `SUCCEEDED`（输出 `{approved: true, comment, decided_by}`）并推进后继；`REJECTED` → 步骤 `FAILED`（`error_code=APPROVAL_REJECTED`）按失败策略处理；`expires_at` 到期由 Cron 扫描置 `EXPIRED` 并按拒绝处理。

---

## 07.4 算子

`Operator` 为 `axpoint-domain` 中对象安全的 trait（方法返回 `Pin<Box<dyn Future<...> + Send>>`），注册表 `OperatorRegistry: HashMap<FlowOperator, Arc<dyn Operator>>`。

| 算子 | `config` | 行为 | 输出 | 幂等 |
| :--- | :--- | :--- | :--- | :--- |
| `SOURCE_INGEST` | `{dataset_id, wait: true, timeout_s}` | 入队 `SYNC_DATASET`，`wait=true` 时轮询至终态 | `{job_id, rows_written, snapshot_id}` | 同步任务唯一索引保证同一数据集不并发 |
| `SQL_TRANSFORM` | `{sql \| saved_query_id, params, materialize_to?: dataset_name, max_rows: ≤1000}` | `AnalyticsQueryPort::run_sql`；`materialize_to` 时创建 / 刷新 `DERIVED_VIEW` 数据集 | `{columns, rows, row_count, truncated, dataset_id?}` | 只读查询本身幂等；物化为 SNAPSHOT 语义 |
| `LLM_EXTRACT` | `{input: "{{...}}", instruction, output_schema (JSON Schema), model?}` | 调用 `axpoint-llm`，要求 JSON 输出并按 `output_schema` 校验，失败重试 1 次 | 校验后的 JSON | 相同输入的结果缓存于步骤日志，重试时复用 |
| `XBLUE_MAP` | `{mapping_id, mode}` | 入队 `MAP_MATERIALIZE` 并等待 | `xb_mapping_runs` 统计 | 映射本身幂等 |
| `AGG_TRIGGER` | `{type_name?}` | 入队 `GRAPH_AGGREGATE` 并等待 | `{nodes_changed}` | 重算幂等 |
| `CAUSAL_SIM` | `{sandbox_id \| scope, actions, compare_with?: scenario_id}` | 调用 AxSim 推演（Step 07 对接仓库已有 `/simulate` 逻辑，Step 10 切换为完整推演端口） | `{scenario_id?, impacts_top, affected_count}` | 推演无副作用 |
| `HUMAN_APPROVAL` | `{title, detail_template, assignee_roles, assignee_users, expires_in_minutes}` | 创建 `ax_approvals` 并返回 `Suspend` | `{approved, comment, decided_by}` | 以 `(run_id, node_id)` 查重，已存在则复用 |
| `ACTION_EMIT` | `{actions: [{kind: WRITE_NODE_METRIC, node_ref, metric, value} \| {kind: WRITE_NODE_PROPERTY, node_ref, property, value} \| {kind: NOTIFY, channel_id, title, body} \| {kind: WORK_ITEM, title, assignee}], dry_run?}` | `PROD` 环境执行；`TEST` 环境或 `dry_run=true` 只写 `ax_action_log(status=DRY_RUN)`；写指标时 `source_tag=ACTION` | `{done, dry_run, failed}` | `ax_action_log` 唯一键 `<run_id>:<node_id>:<序号>` |
| `HTTP_CALL` | `{method, url, test_url?, headers, body, auth: {kind, credential_id}, expect_status: [200..299], response_path?}` | `net_guard` 校验后发送；带 `Idempotency-Key` 请求头；`TEST` 环境使用 `test_url`，未配置则只记录不发送 | `{status, body(≤1 MB) 或 response_path 提取值}` | 依赖对端遵守 `Idempotency-Key` |
| `AGENT_TASK` | 见 Step 09 §09.6 | — | — | — |

`WORK_ITEM` 动作写入 Step 14 的 `ax_work_items`；在 Step 14 之前该动作校验时报 `UNSUPPORTED_FORMAT`。

---

## 07.5 触发器（`triggers.rs`）

`trigger_cfg.type ∈ TriggerType`：

| 类型 | 配置 | 实现 |
| :--- | :--- | :--- |
| `MANUAL` | — | `POST /flows/{id}/runs` |
| `CRON` | `{cron: "0 */1 * * *", timezone?: "Asia/Shanghai", catch_up: false}` | 调度单例每 30 秒调用 `ax_scheduled_flows()`；到期则 `start_run(trigger_source=CRON, idempotency_key=<计划触发时间>)`；`catch_up=false` 时错过的多次只补一次 |
| `OUTBOX_EVENT` | `{topic, filter?: 表达式}` | 外发订阅者对每个事件调用 `ax_event_flows(topic)`，`filter` 以事件载荷顶层字段为变量求值，真则启动运行，`idempotency_key=outbox:<事件 id>` |
| `WEBHOOK` | `{}`（需设置 `slug`） | `POST /api/axflow/hooks/{tenant_id}/{slug}`，`Authorization: Bearer <秘密>`（创建时返回一次）；请求体作为 `trigger` 载荷，请求头 `Idempotency-Key` 作为幂等键 |
| `API` / `MCP` | 由 Step 09 发布端点启动 | — |
| `RULE` | 由 Step 08 规则动作启动 | — |
| `SIM_APPLY` | 由 Step 10 决策下发启动 | — |

只有部署到 `PROD` 的版本响应 `CRON` / `OUTBOX_EVENT` / `WEBHOOK` / `RULE` / `SIM_APPLY` 触发；`MANUAL` 默认运行 `TEST` 部署（未部署 `TEST` 时运行最新版本）。

---

## 07.6 版本与部署

- 编辑：`PUT /flows/{id}` 修改 `ax_flows` 上的草稿（`nodes`、`edges`、`trigger_cfg`），每次保存执行校验并返回问题列表（允许保存有问题的草稿）。
- 发布：`POST /flows/{id}/versions {note}`（`FLOW_PUBLISH`）——草稿无校验错误时写 `ax_flow_versions(version = max + 1)`。
- 部署：`POST /flows/{id}/deploy {environment, version}`（`TEST` 需 `FLOW_EDIT`，`PROD` 需 `FLOW_PUBLISH`），写 `ax_flow_deployments`。
- 回退：把 `PROD` 部署指向较早版本即可。
- 运行中的流程不受部署变化影响（运行记录固定版本号）。

---

## 07.7 前端 `AxFlowPage`

| 组件 | 功能 |
| :--- | :--- |
| `FlowCanvas` | `@xyflow/react`；左侧 `OperatorPalette` 拖入算子；连线；边上可编辑 `when`；节点徽标显示最近一次运行状态 |
| `NodeConfigForm` | 按算子 JSON Schema 渲染表单；模板占位符自动补全祖先节点输出字段 |
| `TriggerEditor` | 触发器类型与配置；Webhook 显示 URL 与一次性秘密 |
| `VersionDeployPanel` | 版本列表、`TEST` / `PROD` 当前部署、部署与回退 |
| `RunHistory` / `RunTimeline` | 运行列表（状态、触发来源、耗时）；甘特式时间线显示每个节点的尝试、输入、输出、错误 |
| `ApprovalInbox` | 待我审批列表（按角色与用户），通过 / 拒绝并填写意见 |

Playwright：`e2e/axflow.spec.ts` 打开演示流程，点击运行后时间线出现，审批收件箱出现一条待审批。

---

## 07.8 路由（`/api/axflow` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /operators`（算子清单与 JSON Schema） | `DATA_READ` |
| `GET /flows`、`POST /flows`、`GET /flows/{id}`、`PUT /flows/{id}`、`DELETE /flows/{id}` | `DATA_READ` / `FLOW_EDIT` |
| `POST /flows/{id}/validate` | `FLOW_EDIT` |
| `GET /flows/{id}/versions`、`POST /flows/{id}/versions` | `DATA_READ` / `FLOW_PUBLISH` |
| `POST /flows/{id}/deploy` | `FLOW_EDIT`（TEST）/ `FLOW_PUBLISH`（PROD） |
| `POST /flows/{id}/runs`（`{environment?, version?, payload?, idempotency_key?}` → `202 {run_id}`） | `FLOW_RUN` |
| `POST /flows/{id}/run`（仓库已有路径，等价于 `POST /flows/{id}/runs` 且 `environment=TEST`） | `FLOW_RUN` |
| `GET /flows/{id}/runs`、`GET /runs/{run_id}`、`GET /runs/{run_id}/steps`、`POST /runs/{run_id}/cancel`、`POST /runs/{run_id}/retry`（从失败节点重新执行） | `DATA_READ` / `FLOW_RUN` |
| `POST /flows/{id}/hook`（创建或轮换 Webhook 秘密） | `FLOW_PUBLISH` |
| `POST /hooks/{tenant_id}/{slug}` | Webhook 秘密 |
| `GET /approvals?status=PENDING&mine=true`、`POST /approvals/{id}/decide`（`{decision: APPROVE|REJECT, comment}`） | `DATA_READ` / `APPROVAL_DECIDE` |
| `GET /actions?run_id=` | `DATA_READ` |

审批决定在一个事务中写 `ax_approvals`、外发 `approval.decided` 与审计 `APPROVAL_DECIDE`；决定者必须属于 `assignee_roles` 之一或在 `assignee_users` 中，否则 403 `PERMISSION_DENIED`。

---

## 07.9 Step 07 验收

`scripts/seed/07-axflow.sh`：创建流程 `daily_supply_refresh`（`sync_orders: SOURCE_INGEST` → `map_orders: XBLUE_MAP` → `agg: AGG_TRIGGER` → `check: SQL_TRANSFORM`（`SELECT count(*) AS n FROM demo_orders WHERE status='PAID'`）→ 边 `when: "n > 0"` → `approve: HUMAN_APPROVAL` → `emit: ACTION_EMIT`（写 W-1 指标 `replenish_flag = 1`））；发布版本 1 并部署到 `TEST` 与 `PROD`；另建流程 `cyclic_demo` 草稿（含环，用于校验）。输出 `FLOW_ID`、`CYCLIC_FLOW_ID`。

`scripts/gate/step07.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-flow && cargo test -p axpoint-db flow_repo)

# 环检测
expect_json "cycle" "$(api POST /api/axflow/flows/$CYCLIC_FLOW_ID/validate)" '[.issues[].code] | index("CYCLE_DETECTED") != null'

# 正式运行：运行到审批暂停
RUN=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"PROD","idempotency_key":"gate-1"}' | jq -r .run_id)
wait_run_status "$RUN" WAITING_APPROVAL
expect_eq "idempotent-run" "$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"PROD","idempotency_key":"gate-1"}' | jq -r .run_id)" "$RUN"

# 审批通过 → 动作执行 → 运行成功
APPROVAL=$(api GET '/api/axflow/approvals?status=PENDING' | jq -r --arg r "$RUN" '.items[] | select(.run_id==$r) | .id')
api POST /api/axflow/approvals/$APPROVAL/decide '{"decision":"APPROVE","comment":"ok"}' >/dev/null
wait_run_status "$RUN" SUCCEEDED
expect_json "action-done" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID)" '.metrics.replenish_flag == 1'
expect_json "steps-all"   "$(api GET /api/axflow/runs/$RUN/steps)" '[.items[] | select(.status=="SUCCEEDED")] | length == 6'

# 测试环境不产生副作用
RUN_T=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"TEST"}' | jq -r .run_id)
wait_run_status "$RUN_T" WAITING_APPROVAL
APPROVAL_T=$(api GET '/api/axflow/approvals?status=PENDING' | jq -r --arg r "$RUN_T" '.items[] | select(.run_id==$r) | .id')
api POST /api/axflow/approvals/$APPROVAL_T/decide '{"decision":"APPROVE"}' >/dev/null
wait_run_status "$RUN_T" SUCCEEDED
expect_json "test-dry-run" "$(api GET "/api/axflow/actions?run_id=$RUN_T")" '.items[0].status == "DRY_RUN"'

# 崩溃恢复：执行中杀掉 Core 进程，重启后运行仍完成（只在本地门禁执行，CI 用 testcontainers 等价用例覆盖）
if [[ "${GATE_ALLOW_RESTART:-0}" == "1" ]]; then
  RUN_C=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"TEST"}' | jq -r .run_id)
  restart_core
  wait_run_status "$RUN_C" WAITING_APPROVAL
fi

gate_pass step07
```

`wait_run_status RUN STATUS` 与 `restart_core` 由 `lib.sh` 提供（A5 §2）。

**通过标准**：最后一行 `GATE step07 PASS`；`axpoint-flow` 集成测试覆盖：两个前驱并发完成时后继只执行一次、`ANY_SUCCESS` 汇合、`when` 为假时下游 `SKIPPED`、重试退避、超时、取消、`FAIL_RUN` 与 `CONTINUE`。

---

# Step 08：规则引擎、告警中心与通知渠道

## 08.0 目标、前置与契约

- **目标**：对指标、数据质量与事件定义规则；判定窗口、持续时间、变化率、缺失、统计异常；告警去重、冷却、确认、自动恢复、静默、升级；五类通知渠道（Webhook、邮件、企业微信、钉钉、飞书）可靠投递；规则可触发流程；运营告警页面。
- **前置**：Step 07 通过。
- **契约清单**：`0008_rules_alerts.sql`；`axpoint-flow::rules`（`RuleDef`、`Condition`、`AlertRecord`、`NotifyChannel`）；`docs/API_CONTRACT.md` 的 `/api/axflow/rules*`、`/alerts*`、`/channels*`、`/silences*`；`lib/api/axflow.ts` 扩展。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0008_rules_alerts.sql` | §08.1 |
| 新建 | `axpoint-core/crates/axpoint-flow/src/rules/{mod.rs, condition.rs, evaluator.rs, alerting.rs}` | 规则判定与告警状态机 |
| 新建 | `axpoint-core/crates/axpoint-flow/src/notify/{mod.rs, webhook.rs, email.rs, wecom.rs, dingtalk.rs, feishu.rs, template.rs}` | 通知渠道 |
| 新建 | `axpoint-core/crates/axpoint-db/src/rules_repo.rs` | 持久化 |
| 修改 | `axpoint-core/crates/axpoint-server/src/background.rs` | `RULE_EVAL` 处理器；`node.metrics_changed`、`dataset.synced` 订阅者；缺失判定与升级扫描（调度单例，每 60 秒）；`alert.opened` / `alert.resolved` 通知分发订阅者 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/axflow.rs` | §08.6 路由 |
| 新建 | `xbluenode-web/src/pages/AxAlertsPage.tsx`、`src/components/axalerts/{AlertTable.tsx, AlertDetail.tsx, RuleEditor.tsx, ChannelForm.tsx, SilenceForm.tsx}` | 运营告警页面 |
| 修改 | `xbluenode-web/src/modules/registry.ts` | 注册 `axalerts` |
| 新建 | `scripts/gate/tools/webhook_sink.py` | 门禁用通知接收器（记录请求到 JSONL） |
| 新建 | `scripts/seed/08-rules.sh` | 演示渠道与规则 |
| 新建 | `scripts/gate/step08.sh` | 门禁 |

---

## 08.1 迁移 `deploy/migrations/0008_rules_alerts.sql`

```sql
-- 0008_rules_alerts.sql：规则、告警、告警事件、通知渠道、投递记录、静默

CREATE TABLE IF NOT EXISTS ax_notify_channels (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             TEXT NOT NULL,
    name                  TEXT NOT NULL,
    kind                  TEXT NOT NULL CHECK (kind IN ('WEBHOOK','EMAIL','WECOM','DINGTALK','FEISHU')),
    config                JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 非秘密字段
    credential_id         UUID REFERENCES ds_credentials(id),   -- 秘密字段（URL 中的 key、签名密钥、SMTP 口令）
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled               BOOLEAN NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_rules (
    id                     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id              TEXT NOT NULL,
    name                   TEXT NOT NULL,
    description            TEXT NOT NULL DEFAULT '',
    scope_kind             TEXT NOT NULL CHECK (scope_kind IN ('METRIC','DATASET_QUALITY','EVENT')),
    target                 JSONB NOT NULL,      -- METRIC: {type_name, metric, node_filter?}；DATASET_QUALITY: {dataset_id?}；EVENT: {topic, filter?}
    condition              JSONB NOT NULL,      -- Condition（§08.2）
    severity               TEXT NOT NULL CHECK (severity IN ('INFO','WARN','CRITICAL')),
    title_template         TEXT NOT NULL,
    body_template          TEXT NOT NULL DEFAULT '',
    cooldown_minutes       INT NOT NULL DEFAULT 30 CHECK (cooldown_minutes >= 0),
    resolve_after_minutes  INT NOT NULL DEFAULT 10 CHECK (resolve_after_minutes >= 0),
    escalate_after_minutes INT CHECK (escalate_after_minutes > 0),
    channel_ids            UUID[] NOT NULL DEFAULT '{}',
    escalation_channel_ids UUID[] NOT NULL DEFAULT '{}',
    run_flow_id            UUID REFERENCES ax_flows(id) ON DELETE SET NULL,
    enabled                BOOLEAN NOT NULL DEFAULT TRUE,
    version                INT NOT NULL DEFAULT 1,
    created_by             UUID,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_alerts (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    rule_id        UUID NOT NULL REFERENCES ax_rules(id) ON DELETE CASCADE,
    dedup_key      TEXT NOT NULL,                  -- 规则 id + 对象（节点 id / 数据集 id / 事件键）
    severity       TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','ACK','RESOLVED')),
    title          TEXT NOT NULL,
    body           TEXT NOT NULL DEFAULT '',
    node_id        UUID,
    dataset_id     UUID,
    last_value     DOUBLE PRECISION,
    detail         JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurrences    INT NOT NULL DEFAULT 1,
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_false_at  TIMESTAMPTZ,                    -- 条件最近一次为假的时间（自动恢复计时）
    acked_by       UUID,
    acked_at       TIMESTAMPTZ,
    escalated_at   TIMESTAMPTZ,
    resolved_at    TIMESTAMPTZ,
    resolve_reason TEXT CHECK (resolve_reason IN ('AUTO','MANUAL','RULE_DISABLED'))
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_alerts_active ON ax_alerts (tenant_id, dedup_key) WHERE status <> 'RESOLVED';
CREATE INDEX IF NOT EXISTS idx_ax_alerts_status ON ax_alerts (tenant_id, status, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS ax_alert_events (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  TEXT NOT NULL,
    alert_id   UUID NOT NULL REFERENCES ax_alerts(id) ON DELETE CASCADE,
    event      TEXT NOT NULL CHECK (event IN ('OPENED','RETRIGGERED','ACKED','ESCALATED','RESOLVED','NOTIFIED','NOTIFY_FAILED','SILENCED','FLOW_STARTED')),
    detail     JSONB NOT NULL DEFAULT '{}'::jsonb,
    actor      UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS ax_notify_deliveries (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    alert_id      UUID REFERENCES ax_alerts(id) ON DELETE CASCADE,
    channel_id    UUID NOT NULL REFERENCES ax_notify_channels(id) ON DELETE CASCADE,
    event         TEXT NOT NULL CHECK (event IN ('OPENED','ESCALATED','RESOLVED','TEST','ACTION')),
    status        TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','SENT','FAILED')),
    attempt       INT NOT NULL DEFAULT 0,
    response_code INT,
    error_message TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sent_at       TIMESTAMPTZ,
    UNIQUE (alert_id, channel_id, event)
);

CREATE TABLE IF NOT EXISTS ax_alert_silences (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  TEXT NOT NULL,
    matcher    JSONB NOT NULL,          -- {rule_ids?, severities?, node_ids?, type_names?}
    starts_at  TIMESTAMPTZ NOT NULL,
    ends_at    TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    reason     TEXT NOT NULL,
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_notify_channels','ax_rules','ax_alerts','ax_alert_events','ax_notify_deliveries','ax_alert_silences'
]::regclass[]) AS t;

CREATE OR REPLACE FUNCTION ax_absence_rules()
RETURNS TABLE (tenant_id TEXT, rule_id UUID)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT r.tenant_id, r.id FROM ax_rules r
     WHERE r.enabled AND (r.condition->>'kind' = 'ABSENCE'
        OR EXISTS (SELECT 1 FROM ax_alerts a WHERE a.rule_id = r.id AND a.status <> 'RESOLVED'));
$$;
REVOKE ALL ON FUNCTION ax_absence_rules() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_absence_rules() TO axpoint_app;
```

`ax_absence_rules()` 同时返回存在活动告警的规则，供每分钟扫描执行自动恢复与升级判定。

---

## 08.2 条件（`rules/condition.rs`）

| `condition.kind` | 参数 | 判定（对一个对象，如一个节点的一个指标） |
| :--- | :--- | :--- |
| `THRESHOLD` | `{op: GT|GTE|LT|LTE|EQ|NE, value, for_minutes: 0}` | `for_minutes=0`：最新值满足；`>0`：`[now - for_minutes, now]` 内全部原始点满足且该区间内至少有 1 个点 |
| `CHANGE_RATE` | `{window_minutes, op, pct}` | `(最新值 - window 起点值) / |window 起点值|` 与 `pct` 比较；起点值为 0 时不判定 |
| `ABSENCE` | `{minutes}` | 最近 `minutes` 内没有该指标的任何点（每分钟扫描判定） |
| `ANOMALY_ZSCORE` | `{lookback_points: 96, z: 3.0, min_points: 30}` | 取最新点之前 `lookback_points` 个点的均值与标准差，`|x - μ| / σ ≥ z`；点数不足 `min_points` 或 `σ = 0` 时不判定 |
| `EXPR` | `{expression}` | Step 06 表达式，变量为节点全部指标与 `prop.*`，结果为真即触发 |
| `QUALITY_FAILED` | `{min_severity: WARN|BLOCK}` | `DATASET_QUALITY` 专用：`dataset.synced` 事件中的质量结果存在不低于该级别的失败 |
| `EVENT_MATCH` | `{}` | `EVENT` 专用：事件主题匹配且 `target.filter` 为真 |

`node_filter`（Step 06 表达式）限定规则作用的节点子集，例 `prop.region == '华东'`。

---

## 08.3 判定与告警状态机（`rules/evaluator.rs`、`rules/alerting.rs`）

### 08.3.1 触发判定

| 来源 | 动作 |
| :--- | :--- |
| `node.metrics_changed`（含聚合与派生产生的） | 找出 `scope_kind=METRIC`、`target.type_name` 与 `target.metric` 匹配的启用规则，入队 `RULE_EVAL {rule_id, node_ids}`（幂等键 `<rule_id>:<事件 id>`） |
| `dataset.synced` | `DATASET_QUALITY` 规则 |
| 任意外发事件 | `EVENT` 规则 |
| 每分钟扫描 `ax_absence_rules()` | `ABSENCE` 判定、活动告警的自动恢复与升级判定 |

### 08.3.2 状态迁移（对每个 `(rule, object)`，`dedup_key = <rule_id>:<object_id>`）

```text
条件为真：
  无活动告警 → 若同 dedup_key 最近一次 RESOLVED 距今 < cooldown_minutes：只写事件 RETRIGGERED 到该已恢复告警，不新开
               否则 INSERT ax_alerts(OPEN)，事件 OPENED，外发 alert.opened
  有活动告警 → occurrences+1、last_seen_at=now、last_value、last_false_at=NULL，事件 RETRIGGERED（同一分钟内合并）
条件为假：
  有活动告警 → last_false_at 为空时设为 now；now - last_false_at ≥ resolve_after_minutes 时 RESOLVED(AUTO)，外发 alert.resolved
扫描：
  OPEN 且 escalate_after_minutes 已到且 escalated_at 为空 → escalated_at=now，事件 ESCALATED，向 escalation_channel_ids 投递
规则停用或删除：活动告警 RESOLVED(RULE_DISABLED)
```

- 告警标题与正文模板占位符：`{{rule.name}}`、`{{node.title}}`、`{{node.type_name}}`、`{{metric}}`、`{{value}}`、`{{threshold}}`、`{{severity}}`、`{{dataset.name}}`、`{{link}}`（前端告警详情 URL）。
- `run_flow_id` 不为空时，OPEN 时以 `trigger_source=RULE`、载荷 `{alert_id, rule_id, node_id, value}`、幂等键 `alert:<alert_id>` 启动该流程的 `PROD` 部署，事件 `FLOW_STARTED`。
- 写入 `ax_alerts` 使用唯一部分索引 `uq_ax_alerts_active` 防止并发判定重复开单（冲突时转为 RETRIGGERED 路径）。

---

## 08.4 通知渠道（`notify/`）

| `kind` | `config`（非秘密） | 秘密字段 | 发送 |
| :--- | :--- | :--- | :--- |
| `WEBHOOK` | `{url, headers}` | `signing_secret` | `POST` JSON `{event, alert, rule, tenant_id, sent_at}`；请求头 `X-AxPoint-Signature: sha256=<HMAC(secret, timestamp + "." + body)>`、`X-AxPoint-Timestamp`、`X-AxPoint-Delivery: <delivery_id>` |
| `EMAIL` | `{smtp_host, smtp_port, starttls: true, from, to: []}` | `smtp_username`、`smtp_password` | `lettre` SMTP；HTML 正文由模板渲染，纯文本备选 |
| `WECOM` | `{}` | `webhook_key` | `POST https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=<key>`，`msgtype=markdown` |
| `DINGTALK` | `{at_mobiles: []}` | `access_token`、`sign_secret` | `POST https://oapi.dingtalk.com/robot/send?access_token=..&timestamp=<ms>&sign=<urlencode(base64(HMAC-SHA256(secret, "<ms>\n<secret>")))>`，`msgtype=markdown` |
| `FEISHU` | `{}` | `webhook_url`（飞书自定义机器人的完整 Webhook 地址，含令牌）、`sign_secret` | `POST <webhook_url>`，请求体含 `timestamp`（秒）与 `sign = base64(HMAC-SHA256(key="<ts>\n<secret>", msg=""))`，`msg_type=interactive` 卡片 |

投递（通知分发订阅者 + `ax_notify_deliveries`）：

1. `alert.opened` / `alert.resolved` / 升级时，为每个渠道写一条 `PENDING` 投递（唯一键防重复）。
2. 静默匹配（当前时间在某条 `ax_alert_silences` 区间内且 `matcher` 匹配）→ 不投递，事件 `SILENCED`。
3. 发送前 `net_guard` 校验 URL（`allow_private_network` 由 `GUARD_ADMIN` 设置）；超时 10 秒。
4. 失败按 `30s, 2m, 10m, 30m, 2h` 退避重试 5 次，之后 `FAILED`，事件 `NOTIFY_FAILED`。
5. 渠道响应体中的业务错误码（钉钉 `errcode≠0`、飞书 `code≠0`、企业微信 `errcode≠0`）视为失败。

`POST /api/axflow/channels/{id}/test` 发送一条测试消息（`event=TEST`）并同步返回结果。

---

## 08.5 前端 `AxAlertsPage`（`/alerts`）

- `AlertTable`：按状态 / 严重度 / 规则 / 节点类型筛选；批量确认；实时刷新（Step 11 提供 SSE 后改为推送，本步每 15 秒轮询）。
- `AlertDetail`：时间线（`ax_alert_events`）、指标走势（标出阈值线与触发点）、投递记录、关联流程运行链接；确认 / 手动恢复。
- `RuleEditor`：作用域、目标（类型 + 指标 + 节点过滤）、条件（按 `kind` 切换表单）、严重度、模板、冷却、恢复、升级、渠道、触发流程；“试判定”按钮对当前数据执行一次判定并展示将会产生的告警（不落库）。
- `ChannelForm`、`SilenceForm`。

Playwright：`e2e/axalerts.spec.ts` 打开 `/alerts`，看到演示告警，点击确认后状态变为 `ACK`。

---

## 08.6 路由（`/api/axflow` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /rules`、`POST /rules`、`GET /rules/{id}`、`PUT /rules/{id}`、`DELETE /rules/{id}` | `DATA_READ` / `RULE_EDIT` |
| `POST /rules/{id}/dry-run` | `RULE_EDIT` |
| `GET /alerts`（`?status=&severity=&rule_id=&node_id=`）、`GET /alerts/{id}`、`GET /alerts/{id}/events` | `DATA_READ` |
| `POST /alerts/{id}/ack`、`POST /alerts/{id}/resolve`、`POST /alerts/ack-batch` | `ALERT_ACK` |
| `GET /channels`、`POST /channels`、`PUT /channels/{id}`、`DELETE /channels/{id}`、`POST /channels/{id}/test` | `DATA_READ` / `RULE_EDIT` |
| `GET /silences`、`POST /silences`、`DELETE /silences/{id}` | `DATA_READ` / `ALERT_ACK` |

---

## 08.7 Step 08 验收

`scripts/seed/08-rules.sh`：启动 `python3 scripts/gate/tools/webhook_sink.py --port 18900 --out .gate/webhook.jsonl`（后台）；创建渠道 `gate_sink`（`WEBHOOK`，`url=http://127.0.0.1:18900/hook`，`allow_private_network=true`）；创建规则 `low_inventory`（`METRIC`，`Warehouse.inventory`，`THRESHOLD LT 50`，`resolve_after_minutes=0`，`cooldown_minutes=0`，渠道 `gate_sink`，触发流程 `daily_supply_refresh`）。输出 `LOW_INV_RULE_ID`、`SINK_FILE`。

`scripts/gate/step08.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-flow rules notify && cargo test -p axpoint-db rules_repo)

# 触发：库存写为 10 → 告警 OPEN、通知送达、流程启动
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":10}],\"source_tag\":\"MANUAL\"}" >/dev/null
wait_until "alert-open" 60 "api GET '/api/axflow/alerts?rule_id=$LOW_INV_RULE_ID&status=OPEN' | jq -e '.items | length == 1'"
wait_until "notified"   60 "grep -c '\"event\":\"OPENED\"' $SINK_FILE | grep -qx 1"
expect_eq "signature" "$(python3 scripts/gate/tools/webhook_sink.py --verify $SINK_FILE --secret "$SINK_SECRET")" "OK"
ALERT=$(api GET "/api/axflow/alerts?rule_id=$LOW_INV_RULE_ID&status=OPEN" | jq -r '.items[0].id')
expect_json "flow-started" "$(api GET /api/axflow/alerts/$ALERT/events)" '[.items[].event] | index("FLOW_STARTED") != null'

# 去重：再次写低值不新开告警
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":9}],\"source_tag\":\"MANUAL\"}" >/dev/null
sleep 3
expect_eq "dedup" "$(sql_app default "SELECT count(*) FROM ax_alerts WHERE rule_id='$LOW_INV_RULE_ID' AND status <> 'RESOLVED'")" "1"

# 确认与自动恢复
expect_code "ack" 200 POST /api/axflow/alerts/$ALERT/ack
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":500}],\"source_tag\":\"MANUAL\"}" >/dev/null
wait_until "auto-resolve" 90 "api GET /api/axflow/alerts/$ALERT | jq -e '.status == \"RESOLVED\" and .resolve_reason == \"AUTO\"'"
wait_until "resolved-notified" 60 "grep -c '\"event\":\"RESOLVED\"' $SINK_FILE | grep -qx 1"

gate_pass step08
```

`wait_until NAME TIMEOUT_S CMD` 由 `lib.sh` 提供；`SINK_SECRET` 为种子脚本创建渠道时设置的签名密钥（写入 `.gate/ids.env`）。

**通过标准**：最后一行 `GATE step08 PASS`；`axpoint-flow` 单测覆盖 7 种条件的边界（`for_minutes` 区间内无点不触发、`CHANGE_RATE` 起点为 0、Z 分数 `σ=0`）、冷却、升级、静默、钉钉与飞书签名向量（对照官方文档示例值）。
