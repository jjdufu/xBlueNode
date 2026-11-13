# 04 · AxFlow 真 DAG 编排调度与 Agent/MCP 发布规范（Step 07 ~ Step 08）

本文件定义 **AxFlow（智能编排流水线）** 的真实执行引擎：从可运行的 DAG 算子编排、定时与事件调度、人机协同审批闸门，到将任意业务流一键发布为标准 **REST API** 与 **MCP (Model Context Protocol) Agent Tool**。

---

## Step 07：真 DAG 拓扑执行引擎与算子矩阵（`axpoint-flow`）

### 7.1 Crate 目录结构（`axpoint-core/crates/axpoint-flow/`）

```text
axpoint-core/crates/axpoint-flow/
├── Cargo.toml
└── src/
    ├── lib.rs             # FlowEngine 入口、DAG 校验与 Kahn 拓扑执行器
    ├── context.rs         # RunContext（步骤输出上下文、模板变量 {{steps.id.field}} 解析）
    ├── operators.rs       # 八大内置算子执行器 Trait 与实现
    ├── scheduler.rs       # Tokio Cron 定时调度器 + 数据集变更/Webhook 事件触发器
    └── mcp.rs             # Model Context Protocol (JSON-RPC / SSE) 工具暴露服务
```

### 7.2 数据库 Schema（`deploy/migrations/004_axflow_and_agents.sql`）

```sql
-- 1. 编排流水线定义表
CREATE TABLE IF NOT EXISTS ax_flows (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    nodes           JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{id, kind, label, config, pos_x, pos_y, retry_limit}]
    edges           JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{id, source, target, condition_expr?}]
    trigger_cfg     JSONB NOT NULL DEFAULT '{"type":"MANUAL"}'::jsonb, -- MANUAL | CRON(expr) | DATASET_EVENT(dataset_id) | WEBHOOK
    status          TEXT NOT NULL DEFAULT 'DRAFT',      -- 'DRAFT' | 'PUBLISHED' | 'PAUSED'
    version         INT NOT NULL DEFAULT 1,
    last_run_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. 流水线运行实例表
CREATE TABLE IF NOT EXISTS ax_flow_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    flow_id         UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    trigger_source  TEXT NOT NULL, -- 'MANUAL' | 'CRON' | 'EVENT' | 'API' | 'MCP' | 'SIM_APPLY'
    input_params    JSONB NOT NULL DEFAULT '{}'::jsonb,
    status          TEXT NOT NULL DEFAULT 'PENDING', -- 'PENDING' | 'RUNNING' | 'WAITING_APPROVAL' | 'SUCCEEDED' | 'FAILED' | 'CANCELED'
    step_outputs    JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_message   TEXT,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at     TIMESTAMPTZ
);

-- 3. 单步执行日志与耗时追踪
CREATE TABLE IF NOT EXISTS ax_flow_step_logs (
    id          BIGSERIAL PRIMARY KEY,
    run_id      UUID NOT NULL REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    step_id     TEXT NOT NULL,
    step_kind   TEXT NOT NULL,
    status      TEXT NOT NULL, -- 'RUNNING' | 'WAITING_APPROVAL' | 'SUCCEEDED' | 'FAILED' | 'SKIPPED'
    summary     TEXT NOT NULL DEFAULT '',
    output_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    duration_ms BIGINT NOT NULL DEFAULT 0,
    logged_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 4. 人机协同审批队列（Human-in-the-Loop Gate）
CREATE TABLE IF NOT EXISTS ax_approvals (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    run_id        UUID NOT NULL REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    step_id       TEXT NOT NULL,
    title         TEXT NOT NULL,
    payload       JSONB NOT NULL DEFAULT '{}'::jsonb, -- 展示给审批人的影响预览/变更内容
    decision      TEXT NOT NULL DEFAULT 'PENDING',    -- 'PENDING' | 'APPROVED' | 'REJECTED'
    reviewer      TEXT,
    comment       TEXT,
    decided_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 5. 已发布的 API 与 MCP Tool 端点注册表
CREATE TABLE IF NOT EXISTS ax_published_endpoints (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    flow_id       UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    slug          TEXT NOT NULL,
    expose_api    BOOLEAN NOT NULL DEFAULT TRUE,
    expose_mcp    BOOLEAN NOT NULL DEFAULT TRUE,
    tool_name     TEXT NOT NULL,
    tool_desc     TEXT NOT NULL,
    input_schema  JSONB NOT NULL DEFAULT '{}'::jsonb,
    api_key_hash  TEXT NOT NULL DEFAULT '',
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug),
    UNIQUE (tenant_id, tool_name)
);

CREATE INDEX IF NOT EXISTS idx_ax_flow_runs_flow ON ax_flow_runs (tenant_id, flow_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_ax_approvals_pending ON ax_approvals (tenant_id, decision, created_at DESC);
```

### 7.3 八大核心算子（`operators.rs`）

| 算子 `kind` | 输入配置（`config`） | 实际执行动作 | 产出的 `output_json` |
| :--- | :--- | :--- | :--- |
| `SOURCE_INGEST` | `{connection_id?, dataset_id?, doc_text?}` | 触发连接器增量同步，或把输入文本/文件推入超图 | `{dataset_id, rows_synced, doc_id?}` |
| `SQL_TRANSFORM` | `{dataset_ids, sql, output_dataset_name}` | 经 `sql_guard` 校验后在 DuckDB 执行 SQL，结果写为新 Parquet 数据集 | `{dataset_id, row_count, preview}` |
| `LLM_EXTRACT` | `{input_ref, prompt_template, json_schema}` | 对上游文本或表记录执行结构化 LLM 抽取/清洗/分类 | `{records, model_used}` |
| `XBLUE_MAP` | `{mapping_id}` 或内联映射配置 | 将数据集行写入 `xb_nodes`/`xb_links` 并写血缘 `xb_provenance` | `{upserted_nodes, upserted_links}` |
| `AGG_TRIGGER` | `{node_types?}` | 触发动态聚合引擎重算派生指标并记录时序点 | `{recomputed_nodes}` |
| `CAUSAL_SIM` | `{sandbox_name, actions}` | 在隔离 CoW 沙盘自动试算推演风险，输出影响清单 | `{sandbox_id, affected_count, top_impacts}` |
| `HUMAN_APPROVAL` | `{title, payload_ref}` | 写入 `ax_approvals` 并将 `run.status` 置为 `WAITING_APPROVAL` 挂起执行，等待人工批准后恢复下游节点 | `{approved_by, comment}` |
| `ACTION_EMIT` | `{action_type: "WEBHOOK" \| "WRITE_NODE" \| "EXPORT", target, body_template}` | 调用外部 HTTP Webhook（带签名重试）、写回业务节点状态或触发报告出品任务 | `{http_status?, exported_file_id?}` |

### 7.4 DAG 执行与断点恢复机制
1. **入图校验**：保存/运行前使用 Kahn 算法检测环路与孤立不可达配置；若存在环路立即返回 `400 Bad Request`。
2. **层级并发执行**：对入度为 0 的就绪节点集合使用 `tokio::task::JoinSet` 并发执行；每完成一步立即持久化 `ax_flow_step_logs` 与 `ax_flow_runs.step_outputs`。
3. **条件分支（`condition_expr`）**：边可配置条件（如 `{{steps.sim.affected_count}} > 0`），不满足条件时下游分支标记为 `SKIPPED`。
4. **挂起与恢复**：遇到 `HUMAN_APPROVAL` 节点时立即释放 Worker 协程，待用户调用 `POST /api/axflow/approvals/{id}/decide`（`APPROVED`）后从 `step_outputs` 恢复上下文继续执行剩余未完成拓扑节点。

---

## Step 08：API 自动发布与 Agentic / MCP 能力底座

### 8.1 一键发布流水线为业务 API
通过 `POST /api/axflow/flows/{id}/publish`：
- 生成唯一的 `POST /api/published/{tenant_id}/{slug}` 对外调用入口，使用 `input_schema`（JSON Schema）校验入参，以 `trigger_source='API'` 同步或异步执行流水线并返回 `step_outputs`。
- 若流水线包含 `HUMAN_APPROVAL`，API 返回 `202 Accepted` 与 `run_id`、`approval_id`，供外部系统轮询或接收回调。

### 8.2 Model Context Protocol (MCP) Server 暴露
在 `axpoint-core/crates/axpoint-flow/src/mcp.rs` 实现标准 MCP 协议端点（`POST /api/mcp/{tenant_id}`，支持 `initialize`、`tools/list`、`tools/call`）：
- 自动将所有 `expose_mcp = true` 的已发布 `AxFlow` 注册为可被外部客户端或内部多智能体调用的 MCP Tool；
- 同时内置 4 个核心平台级 MCP Tool：
  1. `axgraph_query_rag`：五路超图知识检索
  2. `axanalytics_ask_data`：DuckDB 智能问数与制表
  3. `xblue_get_node_detail`：查询实时业务节点与时序指标
  4. `axsim_run_whatif`：在隔离沙盘试算 What-If 影响并返回风险评估

### 8.3 多智能体协同编排（AxAgent Solver）
新增内部协同调度模式 `POST /api/agents/solve`：
- 接收复杂业务问题（如“分析华东仓近 7 天缺货原因并给出调拨建议”）；
- 规划 Agent 依次自动组合调用 `axgraph_query_rag`（查制度/日志异常）、`axanalytics_ask_data`（查库存明细表）、`axsim_run_whatif`（模拟跨仓调拨方案）；
- 产出结构化行动提案，若涉及修改生产节点，强制落入 `ax_approvals` 等待人类最终拍板。

### 8.4 Step 07 ~ Step 08 可验证门禁（Verification Gate）

```bash
# 1. 运行数据库迁移与 Flow DAG 单元测试（拓扑检测/条件分支/挂起恢复）
psql "$DATABASE_URL" -f deploy/migrations/004_axflow_and_agents.sql
cargo test -p axpoint-flow
cargo clippy --workspace --all-targets -- -D warnings

# 2. 端到端测试：创建包含 SQL_TRANSFORM -> XBLUE_MAP -> HUMAN_APPROVAL -> ACTION_EMIT 的 4 步流
curl -s -X POST http://127.0.0.1:9995/api/axflow/flows/<FLOW_ID>/run | jq '{run_id, status}'
# 预期状态为 WAITING_APPROVAL

# 3. 人工审批通过并验证下游 ACTION_EMIT 恢复完成
curl -s -X POST http://127.0.0.1:9995/api/axflow/approvals/<APPROVAL_ID>/decide \
  -H "Content-Type: application/json" \
  -d '{"decision":"APPROVED","reviewer":"ops_lead","comment":"同意调拨"}' | jq '{status}'

# 4. 验证 MCP tools/list 与 tools/call 协议响应
curl -s -X POST http://127.0.0.1:9995/api/mcp/default \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | jq '.result.tools | map(.name)'
```
**通过标准**：
1. 含环 DAG 提交保存时被拒绝并返回明确环路节点提示。
2. 4 步测试流在 `HUMAN_APPROVAL` 处准确挂起，批准后从断点继续跑完 `ACTION_EMIT`（不重复执行前面的 `SQL_TRANSFORM` 与 `XBLUE_MAP`），`ax_flow_runs.status` 变为 `SUCCEEDED`。
3. 已发布的 Flow 同时可通过 `/api/published/default/{slug}` 与标准 MCP `tools/call` 成功触发并获得结构化结果。
