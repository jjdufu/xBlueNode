# A1 · 全局约定：命名、路由、页面、公共枚举、错误码与环境变量

本附录是全部 Step 文件共同引用的**唯一事实来源**。Step 文件中出现的枚举值、路由前缀、页面键、错误码、环境变量，必须与本附录一致；如需扩展，先在契约 PR 中修改本附录，再实现代码。

---

## 1. 自主命名词表

| 名称 | 层 | 职责 | 代码落点 |
| :--- | :--- | :--- | :--- |
| **xBlueNode** | 产品 / 前端工作台 | 业务建模、决策交互、出品的统一工作台 | `xbluenode-web/` |
| **AxPoint** | 引擎底座 | Rust 核心服务与 Python Sidecar 的统称 | `axpoint-core/`、`axpoint-parser/` |
| **AxParser** | 解析 | 文件双轨解析（文本轨 + 表格轨） | `axpoint-parser/parsers/` |
| **AxHub** | 接入 | 连接器、同步、数据资产目录、Webhook、消息流 | `axpoint-parser/connectors/`、`routes/axhub.rs` |
| **AxGraph** | 存储 | 超图（`hg_*`）+ 业务节点（`xb_*`）+ 时序 + 图查询 | `axpoint-db` |
| **AxAnalytics** | 分析 | DuckDB / DuckLake OLAP、透视制表、指标语义层、智能问数 | `axpoint-analytics` |
| **AxQuery** | 问答 | 五路超图召回 + RRF + 意图路由（文档 / 数值 / 混合） | `routes/documents.rs`、`axpoint-analytics::nl2sql` |
| **xBlueStudio** | 建模 | 节点类型 / 关系类型 / 映射 / 派生与聚合规则 / 模型版本 | `routes/xblue.rs`、`XBlueStudioPage.tsx` |
| **xBlueGraph** | 图浏览 | 业务图与超图浏览、邻居展开、路径、图算法结果展示 | `XBlueGraphPage.tsx` |
| **AxFlow** | 编排 | DAG 流程、持久化任务队列、触发器、规则与告警 | `axpoint-flow` |
| **AxAgent** | 智能体 | 发布（API / Skill / MCP）、MCP Client、多模型路由、智能体与会话 | `axpoint-agent` |
| **AxSim** | 推演 | 范围克隆沙盘、写时复制覆盖层、因果传导、方案对比、决策下发 | `axpoint-domain::sim`、`routes/axsim.rs` |
| **xBlueView** | 可视与出品 | 孪生驾驶舱、图表、分享、全格式出品 | `XBlueViewPage.tsx`、`axpoint-parser/exporters/` |
| **AxGuard** | 安全 | 认证、令牌、策略、行级安全、脱敏、审计 | `axpoint-security` |
| **AxOps** | 运维与交付 | 备份恢复、保留策略、模板包、演示数据、交付追踪 | `routes/axops.rs`、`scripts/` |

Rust Crate 全集（`axpoint-core/crates/`）：`axpoint-domain`、`axpoint-llm`、`axpoint-db`、`axpoint-server`（仓库已有），`axpoint-security`（Step 00）、`axpoint-analytics`（Step 04）、`axpoint-flow`（Step 07）、`axpoint-agent`（Step 09）。不设其他 Crate。

---

## 2. 路由前缀表（Core，端口 `9995`）

| 模块 | 前缀 | Rust 路由文件 | 前端 API 文件 | 首次出现 |
| :--- | :--- | :--- | :--- | :--- |
| 系统 | `/api/health`、`/api/stats`、`/api/graph/topology` | `routes/system.rs` | `lib/api/client.ts` | 仓库已有 |
| 认证 | `/api/auth/*` | `routes/auth.rs` | `lib/api/auth.ts` | Step 00（开发令牌）/ Step 13（完整） |
| AxQuery / 文档 | `/api/documents/*`、`/api/chat/*` | `routes/documents.rs` | `lib/api/rag.ts` | 仓库已有 |
| AxHub | `/api/axhub/*` | `routes/axhub.rs` | `lib/api/axhub.ts` | Step 02 |
| AxGraph / xBlueStudio | `/api/xblue/*` | `routes/xblue.rs` | `lib/api/xblue.ts` | 仓库已有 |
| AxAnalytics | `/api/analytics/*` | `routes/analytics.rs` | `lib/api/analytics.ts` | Step 04 |
| AxFlow | `/api/axflow/*` | `routes/axflow.rs` | `lib/api/axflow.ts` | 仓库已有 |
| AxAgent | `/api/axagent/*` | `routes/axagent.rs` | `lib/api/axagent.ts` | Step 09 |
| 已发布端点 | `/api/published/{env}/{tenant_id}/{slug}` | `routes/published.rs` | — | Step 09 |
| MCP Server | `/api/mcp` | `routes/mcp.rs` | — | Step 09 |
| AxSim | `/api/axsim/*` | `routes/axsim.rs` | `lib/api/axsim.ts` | 仓库已有 |
| xBlueView | `/api/xblueview/*` | `routes/xblueview.rs` | `lib/api/xblueview.ts` | Step 11 |
| AxGuard | `/api/axguard/*` | `routes/axguard.rs` | `lib/api/axguard.ts` | Step 13 |
| AxOps | `/api/axops/*` | `routes/axops.rs` | `lib/api/axops.ts` | Step 00（任务查询）/ Step 14（完整） |
| 指标 | `/metrics`（只监听 `AXPOINT_METRICS_ADDR`） | `telemetry.rs` | — | Step 00 |

Sidecar（`axpoint-parser`，端口 `8001`，只在内部网络暴露）：`/health`、`/parse`、`/parse/unified`、`/connectors/*`、`/lake/*`、`/export/*`、`/render/chart`。全量接口见 [A4](./A4-API-CONTRACT-INDEX.md)。

路由书写规则：

1. 路径参数用 Axum 0.8 语法 `{id}`；集合名用复数（`/datasets`、`/flows`）；动作用子路径动词（`/run`、`/publish`、`/apply`）。
2. 所有 `/api/*` 业务路由不接受 `tenant_id` 查询参数或请求体字段；租户只从 `AuthPrincipal` 取得（Step 00 起生效）。外部入口分两类，均用各自的令牌鉴权（总表见 [11](./11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.2）：① 路径中显式带 `{tenant_id}` 的已发布端点（`/api/published/{env}/{tenant_id}/{slug}`）与 Webhook 接收（`/api/axhub/webhooks/{tenant_id}/{slug}/ingest`）、流程钩子（`/api/axflow/hooks/{tenant_id}/{slug}`）；② 路径中不带租户、由令牌经 `SECURITY DEFINER` 函数解析租户的分享页（`/api/xblueview/share/{token}/*`，`ax_resolve_share`）与智能体嵌入（`axe_` 令牌，`ax_resolve_agent_embed`）。
3. 列表接口统一分页参数 `?limit=50&cursor=<opaque>`，响应 `{items: [...], next_cursor: string|null}`。`limit` 范围 `1..=500`。

---

## 3. 页面注册表（前端唯一来源）

`xbluenode-web/src/modules/registry.ts` 按本表生成侧栏与路由；`App.tsx` 使用 `react-router` 的 `createBrowserRouter` 生成路由树。

| key | 页面文件 | 路由 | 侧栏标签 | 所属 Step |
| :--- | :--- | :--- | :--- | :--- |
| `dashboard` | `DashboardPage.tsx` | `/` | 系统概览 | 仓库已有 |
| `axhub` | `AxHubPage.tsx` | `/hub` | AxHub · 数据接入 | Step 02 |
| `axingest` | `AxIngestPage.tsx` | `/ingest` | AxIngest · 知识入库 | 仓库已有 |
| `analytics` | `AnalyticsStudioPage.tsx` | `/analytics` | AxAnalytics · 分析制表 | Step 04 |
| `axquery` | `AxQueryPage.tsx` | `/query` | AxQuery · 智能问答 | 仓库已有，Step 04 / 09 扩展 |
| `xbluegraph` | `XBlueGraphPage.tsx` | `/graph` | xBlueGraph · 图浏览 | 仓库已有，Step 03 / 11 扩展 |
| `xbluestudio` | `XBlueStudioPage.tsx` | `/studio` | xBlueStudio · 节点建模 | 仓库已有，Step 05 扩展 |
| `axflow` | `AxFlowPage.tsx` | `/flow` | AxFlow · 编排 | 仓库已有，Step 07 扩展 |
| `axalerts` | `AxAlertsPage.tsx` | `/alerts` | AxFlow · 运营告警 | Step 08 |
| `axagent` | `AxAgentPage.tsx` | `/agents` | AxAgent · 智能体 | Step 09 |
| `axsim` | `AxSimPage.tsx` | `/sim` | AxSim · 因果推演 | 仓库已有，Step 10 扩展 |
| `xblueview` | `XBlueViewPage.tsx` | `/view` | xBlueView · 孪生驾驶舱 | Step 11 |
| `axguard` | `AxGuardPage.tsx` | `/guard` | AxGuard · 权限与审计 | Step 13 |
| `axops` | `AxOpsPage.tsx` | `/ops` | AxOps · 运维与模板 | Step 14 |
| —（公开，无侧栏） | `LoginPage.tsx` | `/login` | — | Step 00 |
| —（公开，无侧栏） | `SharePage.tsx` | `/share/:token` | — | Step 12 |
| —（公开，无侧栏） | `AgentEmbedPage.tsx` | `/share/agent/:token` | — | Step 09 |

---

## 4. 公共枚举

Rust 中定义在 `axpoint-domain/src/enums.rs`（`#[serde(rename_all = "SCREAMING_SNAKE_CASE")]`），Python 中定义在 `axpoint-parser/schemas.py`（`Literal[...]`），TypeScript 中定义在 `xbluenode-web/src/lib/api/enums.ts`（字符串联合类型），PostgreSQL 中以 `CHECK (col IN (...))` 约束。四处取值完全一致。

| 枚举 | 取值 | 使用位置 |
| :--- | :--- | :--- |
| `DataType` | `STRING` `INT64` `FLOAT64` `DECIMAL` `BOOLEAN` `DATE` `TIMESTAMP` `BINARY` `JSON` `LIST` `STRUCT` | 列画像、结果列、属性定义 |
| `PiiCategory` | `NONE` `PHONE` `ID_CARD` `EMAIL` `BANK_CARD` `NAME` `ADDRESS` `SECRET` | 列画像、属性定义、脱敏 |
| `MaskRule` | `NONE` `MASK_PHONE` `MASK_ID_CARD` `MASK_EMAIL` `MASK_BANK_CARD` `MASK_NAME` `MASK_ADDRESS` `HMAC_HASH` `REDACT` | 脱敏策略 |
| `ConnectorType` | 文件类：`FILE_UPLOAD` `S3` `SFTP`；关系库：`POSTGRES` `MYSQL` `MSSQL` `ORACLE` `SQLITE` `CLICKHOUSE` `DAMENG` `KINGBASE` `OPENGAUSS` `OCEANBASE` `TIDB` `DORIS` `STARROCKS` `HIVE`；NoSQL：`MONGODB` `REDIS` `ELASTICSEARCH` `CASSANDRA` `HBASE` `NEO4J`；时序：`INFLUXDB` `TDENGINE` `IOTDB`；消息：`KAFKA` `MQTT` `PULSAR`；接口：`REST_API` `GRAPHQL` `WEBHOOK` | `ds_connections.connector_type` |
| `SourceKind` | `FILE_TABLE` `SQL_TABLE` `NOSQL_COLLECTION` `TIMESERIES` `STREAM` `API_RESOURCE` `WEBHOOK_STREAM` `DERIVED_VIEW` | `ds_datasets.source_kind` |
| `SyncMode` | `SNAPSHOT` `APPEND` `UPSERT` | `ds_datasets.sync_mode` |
| `JobStatus` | `PENDING` `RUNNING` `SUCCEEDED` `FAILED` `CANCELED` `EXPIRED` | `ax_jobs`、`ds_sync_jobs`、`xv_export_jobs` |
| `JobKind` | `SYNC_DATASET` `PARSE_FILE` `FLOW_STEP` `EXPORT` `RULE_EVAL` `ROLLUP` `LAKE_MAINTENANCE` `RETENTION` `CREDENTIAL_ROTATE` `BACKUP` `EMBED_NODES` `MAP_MATERIALIZE` `GRAPH_AGGREGATE` `KNOWLEDGE_SYNC` | `ax_jobs.kind` |
| `OutboxTopic` | `dataset.synced` `node.metrics_changed` `node.upserted` `approval.decided` `sim.applied` `alert.opened` `alert.resolved` `export.finished` | `ax_outbox.topic` |
| `RetentionTarget` | `METRIC_SERIES` `METRIC_ROLLUP_MINUTE` `AUDIT` `EXPORT_FILE` `FLOW_STEP_LOG` `JOB` `OUTBOX` `LAKE_SNAPSHOT` `MESSAGE` `QUERY_HISTORY` `CONVERSATION` | `ax_retention_policies.target` |
| `Cardinality` | `ONE_TO_ONE` `ONE_TO_MANY` `MANY_TO_ONE` `MANY_TO_MANY`（界面显示 `1:1` `1:N` `N:1` `N:M`） | `xb_edge_types.cardinality` |
| `AggFn` | `SUM` `AVG` `COUNT` `COUNT_DISTINCT` `MIN` `MAX` `MEDIAN` `LAST` | 聚合规则、透视 |
| `MetricSourceTag` | `SYNC` `AGGREGATION` `DERIVED` `ACTION` `MANUAL` `ALGORITHM` | `xb_metric_series.source_tag` |
| `RollupGrain` | `MINUTE` `HOUR` `DAY` `MONTH` | `xb_metric_rollups.grain` |
| `FlowOperator` | `SOURCE_INGEST` `SQL_TRANSFORM` `LLM_EXTRACT` `XBLUE_MAP` `AGG_TRIGGER` `CAUSAL_SIM` `HUMAN_APPROVAL` `ACTION_EMIT` `HTTP_CALL` `AGENT_TASK` | `ax_flows.nodes[].kind` |
| `TriggerType` | `MANUAL` `CRON` `OUTBOX_EVENT` `WEBHOOK` `API` `MCP` `RULE` `SIM_APPLY` | `ax_flows.trigger_cfg.type`、`ax_flow_runs.trigger_source` |
| `RunStatus` | `PENDING` `RUNNING` `WAITING_APPROVAL` `SUCCEEDED` `FAILED` `CANCELED` | `ax_flow_runs.status` |
| `StepStatus` | `PENDING` `RUNNING` `WAITING_APPROVAL` `SUCCEEDED` `FAILED` `SKIPPED` | `ax_flow_step_logs.status` |
| `Environment` | `TEST` `PROD` | 发布端点、流程版本 |
| `Severity` | `INFO` `WARN` `CRITICAL` | 规则与告警 |
| `AlertStatus` | `OPEN` `ACK` `RESOLVED` | `ax_alerts.status` |
| `NotifyKind` | `WEBHOOK` `EMAIL` `WECOM` `DINGTALK` `FEISHU` | `ax_notify_channels.kind` |
| `InterventionMode` | `RATIO` `ADD` `ABSOLUTE` | `AxSimAction.mode` |
| `WidgetKind` | `KPI_CARD` `TABLE` `PIVOT_TABLE` `LINE_CHART` `BAR_CHART` `PIE_CHART` `SCATTER` `SANKEY` `HEATMAP` `GAUGE` `RADAR` `GRAPH_TOPOLOGY` `MAP` `FLOOR_PLAN` `ALERT_LIST` `SIM_COMPARE` `MARKDOWN_NOTE` | `xv_widgets.widget_kind` |
| `ExportFormat` | `XLSX` `CSV` `PDF` `DOCX` `PPTX` `MARKDOWN` | `xv_export_jobs.export_format` |
| `Permission` | `DATA_READ` `DATA_SYNC` `DATA_ADMIN` `STUDIO_EDIT` `STUDIO_PUBLISH` `FLOW_EDIT` `FLOW_RUN` `FLOW_PUBLISH` `APPROVAL_DECIDE` `RULE_EDIT` `ALERT_ACK` `AGENT_EDIT` `AGENT_RUN` `SIM_RUN` `SIM_APPLY` `VIEW_EDIT` `VIEW_SHARE` `EXPORT_DATA` `PII_RAW_READ` `GUARD_ADMIN` `OPS_ADMIN` | `sec_roles.permissions` |
| `BuiltinRole` | `ADMIN`（全部权限）`ANALYST` `OPERATOR` `VIEWER` | `sec_roles.name` |
| `AuditAction` | `LOGIN` `LOGOUT` `TOKEN_ISSUE` `TOKEN_REVOKE` `QUERY_SQL` `ASK_DATA` `RAG_ASK` `DATA_SYNC` `FILE_UPLOAD` `SCHEMA_PUBLISH` `FLOW_RUN` `APPROVAL_DECIDE` `MCP_CALL` `AGENT_CHAT` `SIM_APPLY` `EXPORT_FILE` `SHARE_CREATE` `SHARE_ACCESS` `POLICY_CHANGE` `BACKUP_RUN` `TEMPLATE_INSTALL` | `sec_audit_logs.action` |

---

## 5. 错误响应与错误码

所有 Core 与 Sidecar 的错误响应体统一为：

```json
{ "error_code": "DATASET_NOT_FOUND", "message": "dataset 5f0c... not found", "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736" }
```

Rust 侧 `axpoint-domain::AppError` 枚举与 HTTP 状态码映射：

| `AppError` 变体 | HTTP | 典型 `error_code` |
| :--- | :--- | :--- |
| `BadRequest(code, msg)` | 400 | `INVALID_ARGUMENT` `SQL_REJECTED` `CYCLE_DETECTED` `DIV_BY_ZERO` `LIMIT_OUT_OF_RANGE` `SCHEMA_INVALID` `SIM_DIVERGENT` |
| `Unauthorized(code)` | 401 | `TOKEN_MISSING` `TOKEN_INVALID` `TOKEN_EXPIRED` `SIGNATURE_INVALID` |
| `Forbidden(code)` | 403 | `PERMISSION_DENIED` `POLICY_DENIED` |
| `NotFound(code)` | 404 | `ROUTE_NOT_FOUND` `DATASET_NOT_FOUND` `NODE_NOT_FOUND` `FLOW_NOT_FOUND` `JOB_NOT_FOUND` `SHARE_NOT_FOUND`（跨租户访问一律返回 404） |
| `Conflict(code)` | 409 | `NAME_TAKEN` `VERSION_CONFLICT` `SCHEMA_BREAKING_CHANGE` `IDEMPOTENCY_REPLAY` |
| `PayloadTooLarge(code)` | 413 | `UPLOAD_TOO_LARGE` `ARCHIVE_BOMB` |
| `Unsupported(code)` | 415 | `UNSUPPORTED_FORMAT` `UNSUPPORTED_EARLY_OFFICE_FORMAT` |
| `Unprocessable(code, msg)` | 422 | `PARSE_FAILED` `XML_UNSAFE` `PII_POLICY_VIOLATION` |
| `RateLimited` | 429 | `RATE_LIMITED` |
| `Config(msg)` / `Database(msg)` | 500 | `INTERNAL` |
| `Llm(msg)` / `Parser(msg)` / `Upstream(msg)` | 502 | `UPSTREAM_LLM` `UPSTREAM_PARSER` `UPSTREAM_CONNECTOR` |
| `Timeout(code)` | 504 | `QUERY_TIMEOUT` `UPSTREAM_TIMEOUT` |

Step 00 把仓库中 `AppError` 的五个变体（`Config`、`Database`、`Llm`、`Parser`、`BadRequest`）扩展为上表全集，并把 `axpoint-server/src/state.rs` 中的 `HttpError` 序列化改为上述三字段结构。

---

## 6. 身份与租户约定

1. `AuthPrincipal { user_id: Uuid, tenant_id: String, username: String, dept_id: String, roles: Vec<String>, permissions: BTreeSet<Permission>, token_kind: TokenKind }`，由 `axpoint-security` 的 Axum 提取器从 `Authorization: Bearer <token>` 构造。`TokenKind = ACCESS | API_TOKEN | DEV_STATIC | EMBED | SHARE`（`EMBED` 见 Step 09 §09.7，`SHARE` 见 Step 12）。
2. `tenant_id` 格式：`^[a-z0-9_-]{1,64}$`。仓库默认租户为 `default`。
3. 每个数据库事务开头执行 `SELECT set_config('app.tenant_id', $1, true)`；应用连接使用角色 `axpoint_app`（非超级用户、非表属主），PostgreSQL 行级安全据此隔离租户。
4. 开发与门禁环境使用静态令牌 `AXPOINT_DEV_TOKEN`，映射为 `{tenant_id: default, username: dev, roles: [ADMIN]}`；`AXPOINT_ENV=production` 时启动拒绝加载静态令牌。
5. Core 调用 Sidecar 的请求头：`X-AxPoint-Internal-Token`（内部令牌）、`X-AxPoint-Tenant`（当前主体的 `tenant_id`）、`X-AxPoint-Timezone`（`sec_tenants.timezone`）、`traceparent`（W3C 追踪上下文）。Sidecar 只信任携带正确内部令牌的请求中的租户与时区请求头。

---

## 7. 序列化约定

| 类型 | JSON 表示 |
| :--- | :--- |
| 时间 | RFC 3339 UTC，例 `2026-01-31T08:00:00Z`；数据库统一 `TIMESTAMPTZ` |
| `DECIMAL`、`INT64` 超过 2^53、`HUGEINT` | 字符串 |
| UUID | 小写带连字符字符串 |
| 枚举 | `SCREAMING_SNAKE_CASE` 字符串 |
| 浮点非有限值 | 不允许出现；计算产生时返回错误 |

---

## 8. 代码规范摘要

### 8.1 Rust

- 继承 `axpoint-core/Cargo.toml` 的 `[workspace.lints]`：`unsafe_code = "forbid"`、`clippy::all/pedantic = "deny"`、`unwrap_used/expect_used/panic/todo/unimplemented/unreachable = "deny"`、`as_conversions = "deny"`、`indexing_slicing = "deny"`、`missing_docs/missing_errors_doc/missing_panics_doc = "deny"`。
- 端口注入方式（`axpoint-domain/src/ports.rs` 的 trait 方法返回 `impl Future + Send`，不能做成 trait object）：能力 Crate 用**泛型参数**消费端口，`axpoint-server` 的 `AppState` 持有具体类型。

```rust
/// 编排引擎，只依赖端口，不依赖具体存储。
pub struct FlowEngine<R, Q, A>
where
    R: TopologyRepository + FlowRepository,
    Q: AnalyticsQueryPort,
    A: AiPort,
{
    repo: std::sync::Arc<R>,
    analytics: std::sync::Arc<Q>,
    ai: std::sync::Arc<A>,
}
```

- 需要运行时多态的少数位置（算子注册表、连接器注册表），在 `axpoint-domain` 中定义对象安全的 trait，方法返回 `Pin<Box<dyn Future<Output = Result<T, AppError>> + Send + '_>>`（可用 `trait-variant` 生成）。
- 数据库测试一律 `#[sqlx::test(migrations = "../../../deploy/migrations")]`（路径相对于 Crate 的 `Cargo.toml`）。

### 8.2 Python

- Python `3.11+`，`from __future__ import annotations`，Pydantic 2.x 模型，`ruff check` 与 `mypy --strict` 通过。
- SQL 标识符经 `^[A-Za-z_][A-Za-z0-9_$]*$`（可带 `schema.` 前缀）白名单校验并按方言加引号；值一律参数化。
- 日志注册 `SecretFilter`，屏蔽键名匹配 `(?i)password|passwd|secret|token|authorization|api[_-]?key` 的值。

### 8.3 TypeScript

- `strict: true`，禁止 `any` 与 `// @ts-ignore`；接口响应用 `zod` 做运行时校验。
- 视觉：黑白灰 + 科技蓝 `#2563eb`，浅色 / 深色主题均可用。

---

## 9. 文件变更清单模板

每个 Step 文件都以此格式给出变更清单（动作取值：`新建` / `修改` / `删除`）：

```markdown
| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0003_data_fabric.sql` | ds_* 表与 RLS |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/mod.rs` | `.merge(axhub::router())` |
```

---

## 10. 环境变量总表

| 变量 | 默认值 | 使用方 | 说明 |
| :--- | :--- | :--- | :--- |
| `AXPOINT_ENV` | `development` | Core / Sidecar | `development` / `test` / `production` |
| `DATABASE_URL` | `postgres://axpoint_app:axpoint_app_secret@localhost:5432/xbluenode` | Core | 应用角色连接 |
| `MIGRATION_DATABASE_URL` | `postgres://axpoint:axpoint_secret@localhost:5432/xbluenode` | Core / 脚本 | 迁移与属主连接 |
| `AXPOINT_MIGRATE_ON_START` | `true` | Core | 启动时执行嵌入式迁移 |
| `AXPOINT_APP_DB_PASSWORD` | `axpoint_app_secret` | `scripts/dev-up.sh` | 为 `axpoint_app` 设置口令 |
| `AXPOINT_BIND_ADDR` | `0.0.0.0:9995` | Core | HTTP 监听 |
| `AXPOINT_METRICS_ADDR` | `127.0.0.1:9464` | Core | Prometheus 指标监听 |
| `WEB_DIST_DIR` | `../xbluenode-web/dist` | Core | 前端静态目录 |
| `PARSER_BASE_URL` | `http://127.0.0.1:8001` | Core | Sidecar 地址 |
| `AXPOINT_INTERNAL_TOKEN` | 无（≥32 字节随机） | Core / Sidecar | Core → Sidecar 内部鉴权 |
| `AXPOINT_DEV_TOKEN` | 无（≥32 字节随机） | Core / 门禁脚本 | 开发静态令牌 |
| `AXPOINT_DATA_ROOT` | `/var/lib/axpoint` | Core / Sidecar | 数据根目录（staging、lake、exports、backups） |
| `AXPOINT_LAKE_CATALOG_DSN` | `postgres://axpoint_lake:axpoint_lake_secret@localhost:5432/xbluenode` | Sidecar / Core | DuckLake 目录库连接 |
| `AXPOINT_MODEL_PATH` | `/var/lib/axpoint/models` | Sidecar | OCR / 版面模型目录 |
| `AXPOINT_MAX_UPLOAD_MB` | `512` | Core / Sidecar | 上传上限 |
| `AXPOINT_PARSE_TIMEOUT_S` | `300` | Sidecar | 单文件解析硬超时 |
| `AXPOINT_PDF_MAX_PAGES` | `2000` | Sidecar | PDF 解析页数上限 |
| `AXPOINT_WORKERS` | `4` | Core | 每进程任务 Worker 协程数 |
| `AXPOINT_DUCKDB_TENANT_CACHE` | `32` | Core | 租户 DuckDB 实例 LRU 上限 |
| `AXPOINT_DUCKDB_MEMORY` | `2GB` | Core | 每个租户 DuckDB 实例内存上限 |
| `AXPOINT_LLM_CASSETTE` | 空 | Core | 设置后大模型调用按提示哈希回放录制响应（门禁与 CI） |
| `AXPOINT_EVAL_RECORD` / `AXPOINT_EVAL_LIVE` | 空 | 评测测试 | 录制模型响应 / 连接真实模型评测 |
| `AXPOINT_CREDENTIAL_KEYS` | 无，格式 `1:<base64-32B>[,2:<base64-32B>]` | Core | 凭证加密密钥环 |
| `AXPOINT_MASK_KEYS` | 无，格式同上 | Core | `HMAC_HASH` 脱敏密钥环（按租户派生） |
| `AXPOINT_JWT_PRIVATE_KEY_PATH` / `AXPOINT_JWT_PUBLIC_KEYS_DIR` | 无 | Core | EdDSA 签名私钥与公钥目录（`kid` 为文件名） |
| `AXPOINT_OIDC_ISSUER` / `AXPOINT_OIDC_CLIENT_ID` / `AXPOINT_OIDC_CLIENT_SECRET` / `AXPOINT_OIDC_REDIRECT_URL` | 空（不启用） | Core | OIDC 登录 |
| `AXPOINT_OIDC_AUTO_PROVISION` | `false` | Core | OIDC 首次登录自动建用户 |
| `AXPOINT_OIDC_USERNAME_CLAIM` | `preferred_username` | Core | 用户名取值声明 |
| `AXPOINT_OIDC_ROLE_MAP` | 空 | Core | JSON：`{"groups_claim","map","dept_claim"}`，组到角色与部门的映射 |
| `AXPOINT_LAKE_DB_PASSWORD` | `axpoint_lake_secret` | `postgres-init.sh` | 为 `axpoint_lake` 设置口令 |
| `AXPOINT_WEBHOOK_SECRET` | 无 | 种子脚本 / 门禁 | 演示 Webhook 源的 Bearer 令牌 |
| `AXPOINT_MAP_TILE_URL` | 空（只用矢量轮廓底图） | 前端 / Core | XYZ 瓦片模板，设置后地图组件用 `maplibre-gl` 渲染瓦片 |
| `AXPOINT_CHART_WORKERS` | `2` | Sidecar | 服务端图表渲染子进程数 |
| `AXPOINT_PUBLIC_HOST` | 无（生产必填） | 反向代理 / Core | 对外域名，用于证书、分享链接与 OIDC 回调 |
| `AXPOINT_BACKUP_DIR` | `${AXPOINT_DATA_ROOT}/backups` | `scripts/ops/backup.sh` | 备份输出目录 |
| `AXPOINT_BACKUP_KEEP_DAYS` | `14` | `scripts/ops/backup.sh` | 备份保留日数 |
| `AXPOINT_AUDIT_FORCE_KEEP_DAYS` | 空 | `scripts/ops/retention-audit.sh` | 只用于门禁，覆盖审计保留日数 |
| `AXPOINT_REGISTRY` / `AXPOINT_VERSION` / `AXPOINT_PARSER_VERSION` / `AXPOINT_PARSER_VARIANT` | `dex2all` / `v0.1.0` / `v0.1.0` / `full` | `docker-compose*.yml` | 镜像仓库、版本与 Sidecar 变体（`full` / `lite`） |
| `LLM_API_BASE` / `LLM_API_KEY` / `LLM_MODEL` | 见 `deploy/.env.example` | Core | 默认大模型 |
| `EMBED_API_BASE` / `EMBED_API_KEY` / `EMBED_MODEL` / `EMBED_DIM` | 见 `deploy/.env.example` | Core | 默认向量模型 |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | 空（不导出） | Core / Sidecar | OTLP 追踪导出 |
| `RUST_LOG` | `axpoint_server=info,info` | Core | 日志级别 |
| `NAMING_DENYLIST` | 无（不入库） | `scripts/naming-guard.sh` / CI | 命名守卫正则，CI 来自仓库变量 `vars.NAMING_DENYLIST` |
