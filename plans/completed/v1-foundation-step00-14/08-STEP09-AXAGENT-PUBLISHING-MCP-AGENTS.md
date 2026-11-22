# 08 · Step 09：AxAgent 发布、查询即应用、MCP、多模型路由、智能体与多轮工作台

AxAgent 把平台能力以三种形态对外：**发布端点**（保存查询、指标、业务对象类型、流程、智能体以 REST 接口发布到测试 / 正式环境，附 OpenAPI 文档与 Skill 清单）、**MCP Server**（以 Streamable HTTP 暴露为 MCP 工具），以及**智能体**（可调用内置工具、发布端点与外部 MCP 工具的对话式代理，支持多轮会话、多智能体协作与嵌入分享）。底层的大模型调用经过统一的多模型路由。

---

## 09.0 目标、前置与契约

- **目标**：多模型提供方注册、按用途路由与故障切换、用量统计；五类发布端点（含主图业务对象类型 `OBJECT_TYPE`）的测试 / 正式发布与版本固定、入参校验、限流、缓存、OpenAPI；查询即应用卡片；MCP Server 与 MCP Client；智能体定义、版本、工具调用循环、流式输出、人工确认、多智能体协作；多轮工作台；嵌入链接；`AGENT_TASK` 流程算子；个人访问令牌管理。
- **前置**：Step 07、Step 04 通过。
- **契约清单**：`0009_publishing_agents.sql`；`axpoint-agent` 公开类型（`EndpointSpec`、`AgentSpec`、`ToolRef`、`AgentEvent`、`ConversationRecord`）；`axpoint-domain::ports::SimExecutionPort`（供 `BuiltinToolRegistry` 按 `Option<Arc<dyn SimExecutionPort>>` 注入，避免与 Step 10 改动同一文件）；`axpoint-llm::router`（`LlmRouter`、`Purpose`）；`docs/API_CONTRACT.md` 的 `/api/axagent/*`、`/api/published/*`、`/api/mcp`、`/api/auth/tokens*`；`lib/api/axagent.ts`。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0009_publishing_agents.sql` | §09.1 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/ports.rs` | 契约阶段声明 `SimExecutionPort` trait（L7 并行前合入） |
| 新建 | `axpoint-core/crates/axpoint-llm/src/{router.rs, tools.rs, stream.rs, cassette.rs}` | 多模型路由、函数调用、SSE 流解析、录制回放 |
| 新建 | `axpoint-core/crates/axpoint-agent/{Cargo.toml, src/lib.rs}` | Crate 入口 |
| 新建 | `axpoint-core/crates/axpoint-agent/src/{publish.rs, openapi.rs, mcp_server.rs, mcp_client.rs, runtime.rs, builtin_tools.rs, multi_agent.rs, embed.rs}` | 发布、MCP、智能体运行时 |
| 新建 | `axpoint-core/crates/axpoint-flow/src/operators/agent_task.rs` | `AGENT_TASK` 算子 |
| 新建 | `axpoint-core/crates/axpoint-db/src/agent_repo.rs` | 持久化 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/auth.rs` | `GET/POST /api/auth/tokens`、`DELETE /api/auth/tokens/{id}` |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/{axagent.rs, published.rs, mcp.rs}` | 路由 |
| 修改 | `axpoint-core/Cargo.toml` | 成员 `axpoint-agent`；依赖见 A6 Step 09 |
| 新建 | `xbluenode-web/src/pages/AxAgentPage.tsx`、`src/pages/AgentEmbedPage.tsx`、`src/components/axagent/{ProviderTable.tsx, EndpointList.tsx, EndpointPublishDialog.tsx, AppCard.tsx, AgentEditor.tsx, ToolPicker.tsx, ChatWorkbench.tsx, MessageView.tsx, McpClientForm.tsx, EmbedDialog.tsx}`、`src/lib/api/axagent.ts` | 前端 |
| 修改 | `xbluenode-web/src/modules/registry.ts` | 注册 `axagent` 与公开路由 `/share/agent/:token` |
| 新建 | `scripts/seed/09-agents.sh` | 开发用户、令牌、提供方、端点、智能体 |
| 新建 | `scripts/gate/step09.sh` | 门禁 |

---

## 09.1 迁移 `deploy/migrations/0009_publishing_agents.sql`

```sql
-- 0009_publishing_agents.sql：模型提供方与用量、发布端点、MCP 客户端、智能体、会话、消息、嵌入链接

CREATE TABLE IF NOT EXISTS ax_llm_providers (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    base_url      TEXT NOT NULL,                     -- OpenAI 兼容接口根地址
    model         TEXT NOT NULL,
    purposes      TEXT[] NOT NULL CHECK (purposes <@ ARRAY['CHAT','EMBED','NL2SQL','EXTRACT','AGENT','SUMMARIZE']::TEXT[]),
    credential_id UUID REFERENCES ds_credentials(id),
    priority      INT NOT NULL DEFAULT 100,          -- 同一用途内数值小者优先
    temperature   REAL NOT NULL DEFAULT 0.2,
    max_tokens    INT NOT NULL DEFAULT 4096,
    timeout_s     INT NOT NULL DEFAULT 60,
    rpm_limit     INT,
    supports_tools BOOLEAN NOT NULL DEFAULT TRUE,
    embedding_dim INT,
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_llm_usage (
    tenant_id         TEXT NOT NULL,
    provider_id       UUID NOT NULL REFERENCES ax_llm_providers(id) ON DELETE CASCADE,
    usage_date        DATE NOT NULL,
    purpose           TEXT NOT NULL,
    requests          BIGINT NOT NULL DEFAULT 0,
    errors            BIGINT NOT NULL DEFAULT 0,
    prompt_tokens     BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, provider_id, usage_date, purpose)
);

CREATE TABLE IF NOT EXISTS ax_published_endpoints (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    slug            TEXT NOT NULL CHECK (slug ~ '^[a-z0-9-]{3,64}$'),
    environment     TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    kind            TEXT NOT NULL CHECK (kind IN ('SAVED_QUERY','METRIC','FLOW','AGENT','OBJECT_TYPE')),
    target_id       TEXT NOT NULL,                   -- saved_query id / metric name / flow id / agent id / 主图已发布 NodeType.name
    target_version  INT,                             -- PROD 固定版本；TEST 为空表示跟随最新
    target_snapshot JSONB,                           -- PROD 发布时目标定义的快照（保存查询 spec / 指标定义 / NodeTypeSpec）
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    input_schema    JSONB NOT NULL,                  -- JSON Schema（draft 2020-12）
    output_shape    TEXT NOT NULL DEFAULT 'ROWS' CHECK (output_shape IN ('ROWS','OBJECT','TEXT','RUN')),
    required_scope  TEXT NOT NULL DEFAULT 'AGENT_RUN',
    rate_limit_rpm  INT NOT NULL DEFAULT 60,
    cache_ttl_s     INT NOT NULL DEFAULT 0,
    expose_mcp      BOOLEAN NOT NULL DEFAULT FALSE,
    show_as_app     BOOLEAN NOT NULL DEFAULT TRUE,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    published_by    UUID,
    published_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, environment, slug)
);

CREATE TABLE IF NOT EXISTS ax_mcp_clients (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,31}$'),
    url           TEXT NOT NULL,                     -- Streamable HTTP 端点
    credential_id UUID REFERENCES ds_credentials(id),-- {"authorization": "Bearer ..."} 或自定义请求头
    allowed_tools TEXT[] NOT NULL DEFAULT '{}',      -- 空数组 = 全部
    tools_cache   JSONB NOT NULL DEFAULT '[]'::jsonb,
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    last_synced_at TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_agents (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    spec        JSONB NOT NULL,                      -- AgentSpec 草稿（§09.5.1）
    version     INT NOT NULL DEFAULT 0,              -- 最新发布版本
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_agent_versions (
    tenant_id    TEXT NOT NULL,
    agent_id     UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    version      INT NOT NULL,
    spec         JSONB NOT NULL,
    published_by UUID,
    published_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, version)
);

CREATE TABLE IF NOT EXISTS ax_conversations (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    agent_id      UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    agent_version INT NOT NULL,
    user_id       UUID,
    channel       TEXT NOT NULL CHECK (channel IN ('WORKBENCH','EMBED','API','FLOW','MCP')),
    embed_id      UUID,
    title         TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','WAITING_APPROVAL','CLOSED')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_conversations_user ON ax_conversations (tenant_id, user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS ax_messages (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    conversation_id UUID NOT NULL REFERENCES ax_conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('USER','ASSISTANT','TOOL','SYSTEM')),
    content         TEXT NOT NULL DEFAULT '',
    tool_calls      JSONB,                           -- [{id, name, arguments}]
    tool_call_id    TEXT,
    agent_name      TEXT,                            -- 多智能体时标识发言者
    provider_id     UUID,
    prompt_tokens   INT,
    completion_tokens INT,
    latency_ms      INT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_messages_conv ON ax_messages (tenant_id, conversation_id, id);

CREATE TABLE IF NOT EXISTS ax_agent_embeds (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    agent_id        UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    token_hash      TEXT NOT NULL UNIQUE,            -- hex(SHA-256(axe_<32 字节 base62>))
    allowed_origins TEXT[] NOT NULL DEFAULT '{}',
    permissions     TEXT[] NOT NULL DEFAULT ARRAY['DATA_READ','AGENT_RUN'],
    rate_limit_rpm  INT NOT NULL DEFAULT 20,
    expires_at      TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    created_by      UUID NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_llm_providers','ax_llm_usage','ax_published_endpoints','ax_mcp_clients','ax_agents','ax_agent_versions',
    'ax_conversations','ax_messages','ax_agent_embeds'
]::regclass[]) AS t;

CREATE OR REPLACE FUNCTION ax_resolve_agent_embed(p_token_hash TEXT)
RETURNS TABLE (embed_id UUID, tenant_id TEXT, agent_id UUID, created_by UUID, permissions TEXT[],
               allowed_origins TEXT[], rate_limit_rpm INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT e.id, e.tenant_id, e.agent_id, e.created_by, e.permissions, e.allowed_origins, e.rate_limit_rpm
      FROM ax_agent_embeds e
     WHERE e.token_hash = p_token_hash AND e.revoked_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > NOW());
$$;
REVOKE ALL ON FUNCTION ax_resolve_agent_embed(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_agent_embed(TEXT) TO axpoint_app;
```

---

## 09.2 多模型路由（`axpoint-llm::router`）

```rust
pub enum Purpose { Chat, Embed, Nl2Sql, Extract, Agent, Summarize }

pub struct LlmRouter { /* 每租户缓存 Vec<ProviderHandle>，提供方变更时失效 */ }

impl LlmRouter {
    /// 按用途选择提供方并调用；失败（超时、5xx、429、连接错误）时按 priority 切换到下一个。
    pub async fn chat(&self, tenant: &str, purpose: Purpose, req: ChatRequest) -> Result<ChatResponse, AppError>;
    /// 流式调用，返回 AgentEvent 流（token / tool_call 增量）。
    pub async fn chat_stream(&self, tenant: &str, purpose: Purpose, req: ChatRequest)
        -> Result<impl futures::Stream<Item = Result<StreamDelta, AppError>> + Send, AppError>;
    pub async fn embed(&self, tenant: &str, texts: &[String]) -> Result<(Vec<Vec<f32>>, usize), AppError>;
}
```

规则：

1. 候选：该租户 `enabled=true` 且 `purposes` 含该用途的提供方，按 `priority` 升序；租户没有配置时使用环境变量 `LLM_*` / `EMBED_*` 定义的默认提供方（仓库现有 `AiGateway`）。
2. `rpm_limit`：进程内令牌桶，超限时直接尝试下一个候选。
3. 熔断：同一提供方连续 5 次失败后 60 秒内跳过。
4. `Purpose::Agent` 只选择 `supports_tools=true` 的提供方。
5. 用量：每次调用累加 `ax_llm_usage`（按日聚合，`INSERT ... ON CONFLICT DO UPDATE`），并写指标 `axpoint_llm_tokens_total`。
6. `AXPOINT_LLM_CASSETTE` 设置时（门禁、CI），全部调用按“用途 + 规范化请求体”的 SHA-256 查录制文件回放（`cassette.rs`）。
7. 现有 `AiGateway` 的 `chat`、`synthesize_answer`、`extract_triples` 等方法改为经由 `LlmRouter` 调用（分别对应 `Chat`、`Summarize`、`Extract`），Step 04 的问数使用 `Nl2Sql`。

`POST /api/axagent/providers/{id}/test` 发送一条固定提示，返回延迟与模型回显。

---

## 09.3 发布端点与查询即应用（`publish.rs`）

### 09.3.1 发布

`POST /api/axagent/endpoints`（`STUDIO_PUBLISH`；`kind=FLOW` 需 `FLOW_PUBLISH`）：

```json
{"slug": "paid-amount-by-status", "environment": "TEST", "kind": "METRIC", "target_id": "paid_amount",
 "title": "已支付金额（按状态）", "input_schema": null, "rate_limit_rpm": 60, "cache_ttl_s": 30, "expose_mcp": true}
```

- `input_schema` 为空时自动生成：`SAVED_QUERY` → 由 `an_saved_queries.parameters` 生成；`METRIC` → `{dimensions: enum[...], filters: array, time_range: {from, to}, grain: enum}`；`OBJECT_TYPE` → 由主图当前已发布 `NodeTypeSpec` 的属性与指标生成 `{q?: string, keys?: [string], properties?: object, metrics_min?: object, include_links?: [string], limit?: integer(1..200, default 50), cursor?: string}`；`FLOW` → `{payload: object}`；`AGENT` → `{message: string, conversation_id?: string}`。
- `environment=PROD` 时固定目标版本：保存查询、指标与业务对象类型把发布时刻的完整定义写入 `target_snapshot`（指标记录 `an_metrics.version` 到 `target_version`，`OBJECT_TYPE` 记录主图 `xb_schema_versions.version` 到 `target_version` 并快照该 `NodeTypeSpec`），调用时按快照列与字段输出；流程固定为发布时的 `PROD` 部署版本号；智能体固定为最新发布版本号。`TEST` 端点不固定，始终使用目标的当前定义。
- “提升”：`POST /api/axagent/endpoints/{id}/promote` 把 `TEST` 端点复制为 `PROD`（同 slug），固定当前版本。

### 09.3.2 调用 `POST /api/published/{env}/{tenant_id}/{slug}`

1. 鉴权：`Authorization: Bearer axp_...`（个人访问令牌），令牌租户必须等于路径 `tenant_id`，令牌权限包含 `required_scope`；否则 401 / 403。路径 `env` 为 `test` 或 `prod`。
2. 限流：`(endpoint_id, token_id)` 维度令牌桶，超限 429。
3. 入参校验：`jsonschema` 校验请求体，失败 400 `SCHEMA_INVALID`（附错误路径）。
4. 缓存：`cache_ttl_s > 0` 时以 `(endpoint_id, target_version, 规范化入参, 令牌用户)` 为键，进程内 `moka` 缓存。
5. 执行：`SAVED_QUERY` / `METRIC` → `AnalyticsQueryPort`（以令牌用户为主体，列级脱敏照常生效）；`OBJECT_TYPE` → 以令牌用户为主体查询 `xb_nodes`（`type_name = target_id`，受 RLS、Step 13 部门 ABAC 与属性级 `masking` 约束，按 `target_snapshot` 投影属性与指标并可选展开 `include_links` 指定的 1 跳关联对象），返回 `{items, next_cursor}`；`FLOW` → `start_run(trigger_source=API, environment=env)`，响应 `202 {run_id}`，可用 `GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}` 查询状态；`AGENT` → 智能体一次对话（`channel=API`），`Accept: text/event-stream` 时流式返回。
6. 审计：`MCP_CALL` 之外的发布调用写 `QUERY_SQL`（`SAVED_QUERY` / `METRIC` / `OBJECT_TYPE`）/ `FLOW_RUN` / `AGENT_CHAT`，`detail_json.endpoint_slug` 记录端点。

### 09.3.3 OpenAPI 与 Skill 清单

- `GET /api/axagent/endpoints/openapi.json?environment=PROD`：OpenAPI 3.1，每个端点一个 `POST` 操作，请求体为 `input_schema`，安全方案 `bearerAuth`。
- `GET /api/axagent/endpoints/{id}/skill.json`：Skill 清单 `{name, description, input_schema, invoke: {method: "POST", url}, auth: "bearer"}`，供外部智能体平台导入。

### 09.3.4 查询即应用

`show_as_app=true` 的端点在 AxAgent 页面“应用”页签以 `AppCard` 展示：按 `input_schema` 渲染表单（枚举 → 下拉，日期 → 日期选择，数组 → 多选），提交后以当前登录用户调用对应能力（走 Core 内部调用而非公开端点），结果以表格 + 推荐图表展示；可“复制 cURL”。

---

## 09.4 MCP（`mcp_server.rs`、`mcp_client.rs`，`rmcp`）

### 09.4.1 MCP Server `/api/mcp`

- 传输：Streamable HTTP（`POST /api/mcp` JSON-RPC，支持 `Accept: text/event-stream` 流式响应；会话以 `Mcp-Session-Id` 请求头标识，会话状态保存在进程内并在 30 分钟无活动后清理）。
- 鉴权：`Authorization: Bearer axp_...`；租户与权限来自令牌。
- 工具列表（`tools/list`）：
  - 该租户 `environment=PROD`、`expose_mcp=true`、令牌权限满足 `required_scope` 的发布端点（含 `OBJECT_TYPE` 领域对象端点），工具名为 slug 中 `-` 替换为 `_`，`inputSchema` 为端点 `input_schema`；
  - 内置只读工具（令牌具备 `DATA_READ` 时）：`search_knowledge`（AxQuery 文档召回）、`ask_data`（智能问数）、`list_datasets`、`describe_dataset`、`query_objects`（按主图已发布类型、属性与指标查询业务对象及 1 跳关联）、`graph_neighbors`、`get_node`。
- 资源（`resources/list`）：`axpoint://datasets/{name}`（数据集元数据与列）；`axpoint://objects/{type_name}`（主图已发布对象类型定义与示例）；`resources/read` 返回 JSON。
- 每次 `tools/call` 写审计 `MCP_CALL`（工具名、参数摘要、状态）。

### 09.4.2 MCP Client

- `POST /api/axagent/mcp-clients`（`AGENT_EDIT`）登记外部 MCP 服务；`POST /api/axagent/mcp-clients/{id}/sync` 连接并缓存 `tools/list` 结果到 `tools_cache`。
- 智能体调用外部工具时按需建立会话（每次对话复用同一会话），调用超时 60 秒；`allowed_tools` 之外的工具不可选。
- 出站地址经 `net_guard` 校验。

---

## 09.5 智能体运行时（`runtime.rs`）

### 09.5.1 `AgentSpec`

```json
{
  "system_prompt": "你是供应链运营助手……",
  "provider_purpose": "AGENT",
  "provider_id": null,
  "temperature": 0.2,
  "max_steps": 8,
  "memory_turns": 10,
  "tools": [
    {"kind": "BUILTIN", "name": "ask_data"},
    {"kind": "BUILTIN", "name": "search_knowledge"},
    {"kind": "ENDPOINT", "endpoint_id": "…"},
    {"kind": "MCP", "client": "weather", "tool": "get_forecast"},
    {"kind": "AGENT", "agent_id": "…", "as_tool_name": "analyst"}
  ],
  "approval_required_tools": ["start_flow"],
  "knowledge_scope": {"doc_title_prefix": null, "dataset_names": ["demo_orders"]},
  "output_schema": null
}
```

内置工具（`builtin_tools.rs`，均以当前主体权限执行）：

| 工具 | 权限 | 说明 |
| :--- | :--- | :--- |
| `search_knowledge` | `DATA_READ` | AxQuery 五路召回，返回段落与引用 |
| `ask_data` | `DATA_READ` | Step 04 问数，`knowledge_scope.dataset_names` 限定模式链接范围 |
| `run_sql` | `DATA_READ` | 守卫后执行，最多 200 行 |
| `query_metric` | `DATA_READ` | 指标查询 |
| `query_objects` | `DATA_READ` | 按主图已发布对象类型、属性与指标过滤查询业务对象及 1 跳关联 |
| `get_node`、`graph_neighbors`、`node_metric_series` | `DATA_READ` | AxGraph 查询 |
| `run_simulation` | `SIM_RUN` | `BuiltinToolRegistry` 持有 `Option<Arc<dyn SimExecutionPort>>`：注入实现（Step 10 在 `axpoint-server` 装配）时出现在工具清单并调用推演端口，为 `None` 时自动隐藏 |
| `start_flow` | `FLOW_RUN` | 启动流程 `TEST` 或 `PROD` 部署 |
| `list_alerts` | `DATA_READ` | 告警查询 |

### 09.5.2 循环

```text
messages = [system] + 最近 memory_turns 轮历史 + 用户消息
for step in 1..=max_steps:
    resp = router.chat_stream(purpose=AGENT, messages, tools=工具 JSON Schema 列表)
    流式转发 token 事件
    if resp 无 tool_calls: 写 ASSISTANT 消息；若 output_schema 存在则校验（失败时追加一条修正指令再试一次）；结束
    for call in resp.tool_calls（同一轮多个调用并发执行，最多 4 个）:
        if call.name ∈ approval_required_tools:
            创建 ax_approvals(source_kind=AGENT, source_ref=conversation_id)，会话置 WAITING_APPROVAL，发送 approval_required 事件，暂停
        执行工具（超时 60 秒），结果截断到 16 KB（表格保留前 50 行并注明总行数），写 TOOL 消息，发送 tool_result 事件
达到 max_steps：追加系统指令“请基于已有信息给出最终回答”，再调用一次不带工具的生成
```

- 审批通过后（`approval.decided` 订阅者）执行被暂停的工具并继续循环；拒绝则把“用户拒绝执行 <工具>”作为工具结果继续。
- 流式事件（SSE `event:` 名）：`token`、`tool_call`、`tool_result`、`approval_required`、`final`、`error`；每条 `data:` 为 JSON。
- 每轮写 `ax_messages`（含令牌数与延迟）与审计 `AGENT_CHAT`。

### 09.5.3 多智能体协作（`multi_agent.rs`）

- 形态：**主管 + 专家**。`tools` 中 `kind=AGENT` 的条目把另一个智能体暴露为工具 `as_tool_name`，参数 `{task: string}`。主管调用时，以专家的已发布版本启动一次子对话（`channel=API`，不持久化为独立会话，消息写入父会话并标注 `agent_name`），专家完成后其最终回答作为工具结果返回主管。
- 限制：嵌套深度 ≤ 2（专家不能再调用 `kind=AGENT` 工具，发布时校验）；单次用户请求内所有智能体的模型调用总数 ≤ 20。
- 演示：`ops_supervisor`（主管）+ `data_analyst`（`ask_data`、`run_sql`）+ `knowledge_expert`（`search_knowledge`）。

### 09.5.4 版本

`PUT /api/axagent/agents/{id}` 编辑草稿 `spec`；`POST /api/axagent/agents/{id}/publish` 写 `ax_agent_versions(version+1)`。工作台默认使用最新发布版本，可切换“草稿调试”（只对 `AGENT_EDIT` 持有者可见）。

---

## 09.6 `AGENT_TASK` 流程算子

`config`：`{agent_id, version?, input: "{{ nodes.check.output.rows }}", instruction, output_schema?, max_steps?}`。以运行主体启动一次对话（`channel=FLOW`），结果 `{answer, structured?, conversation_id}`；`approval_required_tools` 中的工具在流程中改为创建流程审批（`source_kind=FLOW`）。幂等：以 `idempotency_key` 查找已有对话，已完成则直接返回结果。

---

## 09.7 嵌入链接（`embed.rs`）

- 创建：`POST /api/axagent/agents/{id}/embeds {allowed_origins, permissions, expires_at, rate_limit_rpm}`（`VIEW_SHARE`），`permissions` 必须是创建者权限的子集；返回一次性 `token`（`axe_` 前缀）与链接 `/share/agent/<token>`。
- 公开页面 `AgentEmbedPage`：只含对话窗口；调用 `POST /api/axagent/embed/{token}/chat`（外部入口，不经过 `AuthPrincipal`）：
  1. `ax_resolve_agent_embed(sha256(token))` 定位；不存在或失效返回 404；
  2. 校验 `Origin` 属于 `allowed_origins`（为空时只允许同源）；
  3. 以“嵌入主体”（`tenant_id`、`user_id=created_by`、`permissions`=嵌入权限与创建者当前权限的交集、`token_kind=EMBED`）执行；
  4. 会话以 `channel=EMBED`、`embed_id` 记录，浏览器以 `conversation_id`（存于页面内存）续聊；
  5. 限流按 `(embed_id, 客户端 IP)`。
- 撤销：`DELETE /api/axagent/embeds/{embed_id}`。

`TokenKind` 因此包含 `EMBED` 与 Step 12 的 `SHARE`，A1 §6 中的取值为 `ACCESS | API_TOKEN | DEV_STATIC | EMBED | SHARE`。

---

## 09.8 个人访问令牌

| 方法与路径 | 说明 |
| :--- | :--- |
| `GET /api/auth/tokens` | 当前用户的令牌（不含明文） |
| `POST /api/auth/tokens` | `{name, scopes: [Permission], expires_in_days?: 1..=365}`；`scopes` 必须是当前用户权限子集；返回一次明文 `axp_<prefix8>_<secret32>`；审计 `TOKEN_ISSUE` |
| `DELETE /api/auth/tokens/{id}` | 吊销；审计 `TOKEN_REVOKE` |

开发静态主体的 `user_id` 为全零 UUID（已在 `0002_platform_foundation.sql` 中作为默认租户 `dev` 用户写入 `sec_users` 并绑定 `ADMIN` 角色），可直接通过 `POST /api/auth/tokens` 签发个人访问令牌。

---

## 09.9 路由汇总

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /api/axagent/providers`、`POST /providers`、`PUT /providers/{id}`、`DELETE /providers/{id}`、`POST /providers/{id}/test` | `AGENT_EDIT` |
| `GET /api/axagent/usage?from=&to=` | `AGENT_EDIT` |
| `GET /api/axagent/endpoints`、`POST /endpoints`、`PUT /endpoints/{id}`、`DELETE /endpoints/{id}`、`POST /endpoints/{id}/promote` | `DATA_READ` / `STUDIO_PUBLISH` |
| `GET /api/axagent/endpoints/openapi.json`、`GET /endpoints/{id}/skill.json` | `DATA_READ` |
| `POST /api/axagent/apps/{endpoint_id}/run` | 端点 `required_scope` |
| `POST /api/published/{env}/{tenant_id}/{slug}`、`GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}` | 个人访问令牌 |
| `POST /api/mcp`、`GET /api/mcp`（SSE 通道）、`DELETE /api/mcp`（结束会话） | 个人访问令牌 |
| `GET /api/axagent/mcp-clients`、`POST /mcp-clients`、`DELETE /mcp-clients/{id}`、`POST /mcp-clients/{id}/sync` | `AGENT_EDIT` |
| `GET /api/axagent/agents`、`POST /agents`、`GET /agents/{id}`、`PUT /agents/{id}`、`DELETE /agents/{id}`、`POST /agents/{id}/publish` | `DATA_READ` / `AGENT_EDIT` |
| `POST /api/axagent/agents/{id}/chat`（`{message, conversation_id?, draft?: bool}`；`Accept: text/event-stream` 时流式，`Accept: application/json` 时等待完成后返回 `{conversation_id, answer, messages}`） | `AGENT_RUN` |
| `GET /api/axagent/conversations`、`GET /conversations/{id}/messages`、`DELETE /conversations/{id}` | `AGENT_RUN`（只看本人） |
| `POST /api/axagent/agents/{id}/embeds`、`GET /agents/{id}/embeds`、`DELETE /embeds/{embed_id}` | `VIEW_SHARE` |
| `POST /api/axagent/embed/{token}/chat` | 嵌入令牌 |

---

## 09.10 前端 `AxAgentPage`（`/agents`）

| 页签 | 内容 |
| :--- | :--- |
| 工作台 | `ChatWorkbench`：左侧会话列表，中间消息流（工具调用折叠卡片显示参数与结果，表格结果可展开、图表可渲染，多智能体消息标注发言者），审批请求内联卡片；右侧智能体信息与引用 |
| 智能体 | `AgentEditor`：系统提示、模型用途 / 指定提供方、`ToolPicker`（内置 / 端点 / MCP / 智能体）、需审批工具、知识范围、输出 Schema；草稿调试与发布；嵌入链接管理（`EmbedDialog`） |
| 应用 | `AppCard` 网格（查询即应用） |
| 发布 | `EndpointList`：测试 / 正式两列、提升、OpenAPI 下载、Skill 清单、cURL 示例 |
| 模型 | `ProviderTable`：提供方、用途、优先级、测试、用量图 |
| MCP | 外部 MCP 客户端登记与工具同步；本平台 MCP Server 地址与客户端配置示例 |

Playwright：`e2e/axagent.spec.ts` 在工作台向演示智能体提问，消息流出现 `ask_data` 工具卡片与最终回答（门禁环境使用录制响应）。

---

## 09.11 验收

`scripts/seed/09-agents.sh`：创建个人访问令牌 `gate`（全部权限）写入 `.gate/ids.env` 的 `GATE_PAT`；（门禁环境不注册提供方，路由回落到环境变量默认提供方，并由 `AXPOINT_LLM_CASSETTE` 回放录制响应）；发布端点 `paid-amount-by-status`（`METRIC`，`TEST` 并提升到 `PROD`，`expose_mcp=true`）与 `warehouse-objects`（`OBJECT_TYPE`，`target_id="Warehouse"`，`TEST` 并提升到 `PROD`，`expose_mcp=true`）；创建并发布智能体 `ops_supervisor`、`data_analyst`、`knowledge_expert`；输出 `PAID_ENDPOINT_SLUG`、`OBJECT_ENDPOINT_SLUG`、`SUPERVISOR_AGENT_ID`。

`scripts/gate/step09.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-agent && cargo test -p axpoint-llm router cassette)

PUB="$CORE_BASE_URL/api/published/prod/default/$PAID_ENDPOINT_SLUG"
OBJ_PUB="$CORE_BASE_URL/api/published/prod/default/$OBJECT_ENDPOINT_SLUG"
pat() { curl -s -H "Authorization: Bearer $GATE_PAT" -H "Content-Type: application/json" "$@"; }

# 发布端点：令牌鉴权、入参校验、指标与业务对象结果
expect_eq   "pub-noauth" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$PUB" -d '{}')" "401"
expect_json "pub-invalid" "$(pat -X POST "$PUB" -d '{"grain":"WEEKLY"}')" '.error_code == "SCHEMA_INVALID"'
expect_json "pub-ok"      "$(pat -X POST "$PUB" -d '{"dimensions":["status"],"grain":"MONTH"}')" '(.rows | length) >= 1'
expect_json "published-object" "$(pat -X POST "$OBJ_PUB" -d '{"keys":["W-1"],"include_links":["FULFILLS"]}')" '(.items | length) == 1 and .items[0].node_key == "W-1"'
expect_json "openapi"     "$(api GET '/api/axagent/endpoints/openapi.json?environment=PROD')" '.paths | keys | (any(test("paid-amount-by-status")) and any(test("warehouse-objects")))'

# PROD 固定版本：修改指标后 PROD 结果不变，TEST 跟随
BEFORE=$(pat -X POST "$PUB" -d '{"dimensions":[],"grain":"MONTH"}' | jq -c .rows)
api PATCH /api/analytics/metrics/paid_amount '{"filter_sql":"status = '\''PENDING'\''"}' >/dev/null
expect_eq "prod-pinned" "$(pat -X POST "$PUB" -d '{"dimensions":[],"grain":"MONTH"}' | jq -c .rows)" "$BEFORE"
api PATCH /api/analytics/metrics/paid_amount '{"filter_sql":"status = '\''PAID'\''"}' >/dev/null

# MCP：初始化、工具列表、调用
INIT=$(pat -X POST "$CORE_BASE_URL/api/mcp" -H "Accept: application/json, text/event-stream" -D .gate/mcp.h \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gate","version":"1"}}}')
SID=$(awk 'tolower($1)=="mcp-session-id:" {gsub(/\r/,"",$2); print $2}' .gate/mcp.h)
mcp() { pat -X POST "$CORE_BASE_URL/api/mcp" -H "Accept: application/json, text/event-stream" -H "Mcp-Session-Id: $SID" -d "$1" | sed -n 's/^data: //p;/^{/p' | tail -1; }
mcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true
expect_json "mcp-tools" "$(mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list"}')" '[.result.tools[].name] | (index("paid_amount_by_status") != null and index("warehouse_objects") != null and index("query_objects") != null)'
expect_json "mcp-call"  "$(mcp '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"paid_amount_by_status","arguments":{"dimensions":["status"],"grain":"MONTH"}}}')" '.result.isError != true'
expect_eq "mcp-audit" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='MCP_CALL'")" "t"

# 智能体（录制响应）：多智能体协作产生专家发言
CONV=$(api POST /api/axagent/agents/$SUPERVISOR_AGENT_ID/chat '{"message":"已支付订单总金额是多少？依据哪份制度补货？"}' | jq -r .conversation_id)
expect_json "multi-agent" "$(api GET /api/axagent/conversations/$CONV/messages)" '[.items[].agent_name] | (index("data_analyst") != null and index("knowledge_expert") != null)'

# 嵌入链接：令牌外部访问、撤销后失效
EMB=$(api POST /api/axagent/agents/$SUPERVISOR_AGENT_ID/embeds '{"allowed_origins":[],"permissions":["DATA_READ","AGENT_RUN"]}')
ETOKEN=$(echo "$EMB" | jq -r .token); EID=$(echo "$EMB" | jq -r .id)
expect_code "embed-ok" 200 POST_NOAUTH "/api/axagent/embed/$ETOKEN/chat" '{"message":"你好"}'
api DELETE /api/axagent/embeds/$EID >/dev/null
expect_code "embed-revoked" 404 POST_NOAUTH "/api/axagent/embed/$ETOKEN/chat" '{"message":"你好"}'

gate_pass step09
```

`expect_code` 的方法参数 `POST_NOAUTH` 表示不带 `Authorization` 请求头（A5 §2）。

**通过标准**：最后一行 `GATE step09 PASS`；`axpoint-llm` 路由测试覆盖优先级、故障切换、熔断、`rpm_limit`；`axpoint-agent` 测试覆盖工具并发、`max_steps` 收尾、审批暂停与恢复、嵌套深度校验。
