# xBlueNode / AxPoint 接口契约（API Contract）

本文件记录**已合入 `main` 的接口**，每个 Step 的契约 PR 把该 Step 的接口条目写入对应小节。全部接口（含各 Step 计划交付的接口）的方法、路径、权限与定义位置见 [blueprint/A4-API-CONTRACT-INDEX.md](./blueprint/A4-API-CONTRACT-INDEX.md)；前缀、分页、错误码、序列化约定见 [blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md](./blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 0. 通用约定

- **鉴权**：除 `GET /api/health` 与 Step 13 §13.2 列出的外部入口外，所有 `/api/*` 请求携带 `Authorization: Bearer <token>`。Step 00 起令牌为开发静态令牌 `AXPOINT_DEV_TOKEN`；Step 09 起支持个人访问令牌（`axp_` 前缀）；Step 13 起支持 EdDSA 访问令牌（有效期 900 秒）与刷新令牌。
- **租户**：租户只取自令牌对应的主体；请求中出现 `tenant_id` 查询参数或请求体字段时返回 `400 INVALID_ARGUMENT`。外部入口的租户取自路径或令牌（A1 §2 规则 2）。
- **错误体**：`{"error_code": "...", "message": "...", "trace_id": "..."}`，状态码与 `error_code` 对照见 A1 §5；跨租户访问一律返回 `404`。
- **分页**：`?limit=50&cursor=<opaque>`，响应 `{items: [...], next_cursor: string | null}`，`limit` 范围 `1..=500`。
- **序列化**：JSON 字段 `snake_case`，时间为 RFC 3339 UTC；实体与超图主键（`hg_*`、`xb_*`、`ds_*` 等）统一为 UUID 字符串，仅追加型事件与审计流水表（`sec_audit_logs`、`ax_outbox`、`ax_flow_step_logs`、`ax_action_log`、`ax_alert_events`、`ax_messages`、`ax_work_item_events`）主键为 `BIGINT`。

---

## 1. AxPoint Core（`axpoint-core`，端口 `9995`）

### `GET /api/health`
公开。返回服务状态与当前 Embedding 维度探测结果。

### `GET /api/stats`
权限 `DATA_READ`。返回 `Doc / Para / Entity / Edge / Embedding` 计数与已存储的向量维度列表 `embedding_dims`。

### `POST /api/documents/ingest`
权限 `DATA_SYNC`。提交 Markdown / 纯文本构建三级超图：

```json
{ "title": "文档标题", "source_name": "doc.md", "content": "正文内容..." }
```

### `POST /api/documents/upload`
权限 `DATA_SYNC`。`multipart/form-data`（字段 `file`），Core 转发至 Sidecar 解析为 Markdown 后入库。

### `POST /api/chat/query`
权限 `DATA_READ`。五路召回（Vector / BM25 / EntityLink / Context / MultiHop）+ RRF 融合 + 模型合成回答：

```json
{ "query": "什么是动态超图架构？", "top_k": 6 }
```

### `GET /api/graph/topology`
权限 `DATA_READ`。返回 xBlueGraph 所需的 `nodes` 与 `edges`。

### `GET /api/xblue/schema` / `POST /api/xblue/schema`
权限 `DATA_READ` / `STUDIO_PUBLISH`。读取或写入业务节点类型（`BlueNodeTypeDef`）与关系类型（`BlueEdgeTypeDef`）。Step 05 起支持多子图工作空间（`/api/xblue/subgraphs*`），增量编辑走草案 → 发布流程（`/api/xblue/drafts/*`），模型发布后自动暴露领域对象 REST（`/api/xblue/objects*`）与动态 GraphQL（`/api/xblue/graphql*`）。

### `GET /api/xblue/nodes` / `POST /api/xblue/nodes`
权限 `DATA_READ` / `STUDIO_EDIT`。查询或写入业务节点（`BlueNode`）与关系（`BlueLink`）。

### `GET /api/axflow/flows` / `POST /api/axflow/flows`
权限 `DATA_READ` / `FLOW_EDIT`。查询或创建 AxFlow 流程。

### `POST /api/axflow/flows/{id}/run`
权限 `FLOW_RUN`。执行流程并返回步骤日志。Step 07 起该路径等价于 `POST /api/axflow/flows/{id}/runs {environment: "TEST"}`，返回 `run_id`，结果经 `GET /api/axflow/runs/{run_id}` 查询。

### `GET /api/axsim/sandboxes` / `POST /api/axsim/sandboxes`
权限 `DATA_READ` / `SIM_RUN`。列出或创建推演沙盘。

### `POST /api/axsim/sandboxes/{id}/simulate`
权限 `SIM_RUN`。施加干预（`target_node_id`、`metric_key`、`delta_ratio`），返回多跳传导影响明细。Step 10 起 `delta_ratio` 作为 `AxSimAction { mode: RATIO }` 的简写保留其语义。

### `GET /api/auth/me`
已登录。返回当前主体 `{tenant_id, username, roles, permissions}`。

### `GET /api/axops/jobs` / `GET /api/axops/jobs/{id}`
权限 `DATA_READ`。后台任务列表与详情（`JobStatus`：`PENDING` `RUNNING` `SUCCEEDED` `FAILED` `CANCELED` `EXPIRED`）。

### `AxHub` 数据接入、同步与资产目录（`/api/axhub/*`，Step 02）

- `GET /api/axhub/connector-types`（`DATA_READ`）：返回 32 种 `ConnectorType` 规格卡片（含 `available`、`config_schema`、`secret_fields`、`supports_modes`）。
- `GET /api/axhub/connections` / `POST /api/axhub/connections`（`DATA_READ` / `DATA_ADMIN`）：列表或创建连接；秘密字段拆分加密存入 `ds_credentials`，响应仅返回 `secret_fields_set`（永不回显秘密）。
- `GET /api/axhub/connections/{id}` / `PATCH /api/axhub/connections/{id}` / `DELETE /api/axhub/connections/{id}`（`DATA_READ` / `DATA_ADMIN`）：读取、增量更新或删除连接（存在关联数据集时返回 `409`）。
- `POST /api/axhub/connections/{id}/test` / `GET /api/axhub/connections/{id}/discover`（`DATA_ADMIN`）：连接测试（返回 `{ok, message, latency_ms}`）与远端对象发现（返回 `DiscoveredObject[]`）。
- `POST /api/axhub/connections/{id}/preview`（`DATA_READ`）：远端对象样本预览（`{ref, limit}`）。
- `POST /api/axhub/credentials/rotate`（`DATA_ADMIN`）：入队 `CREDENTIAL_ROTATE` 任务，使用当前最大版本密钥重新加密 `ds_credentials`。
- `GET /api/axhub/datasets` / `POST /api/axhub/datasets`（`DATA_READ` / `DATA_SYNC`）：查询或创建数据集（支持 `?q=&tag=&source_kind=&limit=&cursor=`）。
- `GET /api/axhub/datasets/{id}` / `PATCH /api/axhub/datasets/{id}` / `DELETE /api/axhub/datasets/{id}`（`DATA_READ` / `DATA_SYNC` / `DATA_ADMIN`）：数据集详情、更新与删除（同时清理 DuckLake 表）。
- `POST /api/axhub/datasets/{id}/sync`（`DATA_SYNC`）：单飞入队 `SYNC_DATASET`（新建返回 `202 {job_id}`，已有待执行/运行中任务返回 `200 {job_id, existing: true}`）。
- `GET /api/axhub/datasets/{id}/sync-jobs` / `GET /api/axhub/datasets/{id}/preview` / `GET /api/axhub/datasets/{id}/snapshots`（`DATA_READ`）：同步历史、湖表预览（支持 `?snapshot_id=`）与快照列表。
- `GET /api/axhub/datasets/{id}/columns` / `PUT /api/axhub/datasets/{id}/columns/{column}`（`DATA_READ` / `DATA_ADMIN`）：列级目录元数据与人工 PII 确认。
- `GET /api/axhub/datasets/{id}/quality-rules` / `POST /api/axhub/datasets/{id}/quality-rules` / `DELETE /api/axhub/quality-rules/{rule_id}`（`DATA_READ` / `DATA_ADMIN`）：数据质量规则管理。
- `GET /api/axhub/datasets/{id}/lineage`（`DATA_READ`）：上下游资产血缘图（`?depth=3`）。
- `POST /api/axhub/uploads` / `GET /api/axhub/uploads/{id}`（`DATA_SYNC` / `DATA_READ`）：统一文件上传（SHA-256 去重、`PARSE_FILE` 双轨入知识库与入湖）。
- `POST /api/axhub/webhooks` / `GET /api/axhub/webhooks` / `DELETE /api/axhub/webhooks/{id}`（`DATA_ADMIN`）：Webhook 创建（一次性返回随机秘密）、列表与删除。
- `POST /api/axhub/webhooks/{tenant_id}/{slug}/ingest`（外部入口，`HMAC_SHA256` 或 `Bearer` 签名校验）：幂等写入 `ds_webhook_buffer`（新投递 `202`，重复 `delivery_id` 返回 `200`）。
- `GET /api/axhub/catalog/search` / `GET /api/axhub/glossary` / `POST /api/axhub/glossary` / `DELETE /api/axhub/glossary/{id}`（`DATA_READ` / `DATA_ADMIN`）：全文与同义词扩展资产目录检索及业务术语表管理。

### `AxGraph` 存储扩展、时序与图查询（Step 03）

- `GET /api/xblue/nodes`（`DATA_READ`）：支持 `?type_name=&q=&bbox=minLng,minLat,maxLng,maxLat&limit=&cursor=`，返回 `{items, nodes, links, next_cursor}`。
- `POST /api/xblue/nodes`（`STUDIO_EDIT`）：幂等 upsert 业务节点（支持 `properties`、`metrics_merge: MERGE|REPLACE`、`geo`、乐观锁 `if_row_version`、关联边创建），入队 `EMBED_NODES` 并写 `ax_outbox`。
- `POST /api/xblue/nodes/bulk`（`STUDIO_EDIT`）：单批最多 5 000 个节点 + 20 000 条关系（`UNNEST` 批量写入，端点支持 `{type_name, node_key}` 解析）。
- `GET /api/xblue/nodes/{id}` / `PATCH /api/xblue/nodes/{id}` / `DELETE /api/xblue/nodes/{id}`（`DATA_READ` / `STUDIO_EDIT`）：单节点查询、增量更新与级联删除（连同 `xb_links` 与 `xb_metric_series`）。
- `GET /api/xblue/nodes/search?q=&type_name=&k=20`（`DATA_READ`）：PostgreSQL FTS + `pgvector` HNSW 双通道 RRF（`k=60`）混合检索。
- `GET /api/xblue/nodes/{id}/documents`（`DATA_READ`）：经 `xb_entity_links` 与 `MENTIONS` 超边查询提及该节点的文档与段落摘录。
- `GET /api/xblue/links` / `POST /api/xblue/links` / `DELETE /api/xblue/links/{id}`（`DATA_READ` / `STUDIO_EDIT`）：业务关系列表、幂等 upsert（唯一键 `(tenant_id, link_type, src_node_id, dst_node_id)`）与删除。
- `POST /api/xblue/metrics/write`（`DATA_SYNC`）：批量写入时序原始点（最多 50 000 点），同步更新 `xb_nodes.metrics` 最新值与 `xb_rollup_dirty` 迟到桶，并入队 `ROLLUP`。
- `GET /api/xblue/nodes/{id}/metrics/{metric}?from=&to=&grain=&agg=`（`DATA_READ`）：自动或显式粒度时序查询（超过 5 000 点自动升粒度）。
- `POST /api/xblue/metrics/window` / `GET /api/xblue/metrics/catalog`（`DATA_READ`）：滚动/滑动窗口聚合查询与租户指标目录。
- `GET /api/xblue/graph/neighbors` / `GET /api/xblue/graph/path` / `GET /api/xblue/graph/subgraph`（`DATA_READ`）：防环递归 CTE k 跳邻居、双向 BFS / Dijkstra 最短路径与画布子图。
- `POST /api/xblue/graph/algorithms/{algo}` / `GET /api/xblue/graph/algorithms/{algo}/results`（`STUDIO_EDIT` / `DATA_READ`）：`PAGERANK`、`DEGREE`、`LABEL_PROPAGATION`、`CONNECTED_COMPONENTS` 图算法运行与分页结果读取。
- `POST /api/xblue/entities/resolve` / `GET /api/xblue/entities/candidates` / `POST /api/xblue/entities/links/{entity_id}/{node_id}/confirm|reject`（`STUDIO_EDIT`）：超图实体与业务节点归一化自动对齐与人工确认/拒绝。
- `POST /api/axops/jobs/rollup-now`（`OPS_ADMIN`）：为当前租户立即入队一次 `ROLLUP` 任务并返回 `{job_id}`。

### `AxAnalytics` OLAP、透视制表、指标语义层与智能问数（`/api/analytics/*` 与 `/api/chat/query` 扩展，Step 04）

- `GET /api/analytics/catalog`（`DATA_READ`）：返回当前租户在湖中的数据集与列目录（含 PII 与业务术语）、`ax.*` 平台联邦视图及 `live_*` 直连对象树。
- `POST /api/analytics/sql/run` / `POST /api/analytics/sql/explain`（`DATA_READ`）：经 `sql_guard` AST 白名单与租户表改写后执行或解释只读 `DuckDB` SQL；默认对所有主体动态脱敏 PII 列（除非 `reveal_pii=true` 且具备 `PII_RAW_READ`），并记录 `an_query_history` 与 `QUERY_SQL` 审计。
- `POST /api/analytics/pivot`（`DATA_READ`）：拖拽式透视制表编译与交叉矩阵化（支持 `GROUPING SETS` 小计/总计、列组合频次上限与非可加聚合截断提示）。
- `GET /api/analytics/metrics` / `POST /api/analytics/metrics` / `GET /api/analytics/metrics/{name}` / `PATCH /api/analytics/metrics/{name}` / `DELETE /api/analytics/metrics/{name}`（`DATA_READ` / `STUDIO_EDIT`，`certified=true` 需 `STUDIO_PUBLISH`）：指标语义层管理、聚合表达式校验、破坏性 Schema 检测（`broken=true`）与血缘记录。
- `POST /api/analytics/metrics/{name}/query` / `POST /api/analytics/metrics/query-multi`（`DATA_READ`）：单指标或多指标按维度、时间粒度与同比/环比（`PREVIOUS_PERIOD` / `SAME_PERIOD_LAST_YEAR`）编译查询。
- `GET /api/analytics/saved-queries` / `POST /api/analytics/saved-queries` / `GET /api/analytics/saved-queries/{id}` / `PATCH /api/analytics/saved-queries/{id}` / `DELETE /api/analytics/saved-queries/{id}` / `POST /api/analytics/saved-queries/{id}/run`（`DATA_READ` / `VIEW_EDIT`）：参数化保存查询管理与运行。
- `POST /api/analytics/ask` / `POST /api/analytics/ask/{turn_id}/feedback` / `GET /api/analytics/ask/sessions/{session_id}`（`DATA_READ`）：智能问数管线（意图路由、FTS+向量模式链接、`sql_guard` + `EXPLAIN` 自修复、数值逐字核验摘要、图表建议、多轮会话与点赞/修正少样本反馈）。
- `POST /api/analytics/transform/draft` / `PATCH /api/analytics/transform/{id}` / `POST /api/analytics/transform/{id}/apply` / `POST /api/analytics/transform/{id}/discard`（`DATA_SYNC`）：自然语言 → `SELECT` 变换草案 → 100 行脱敏预览 → 确认物化为 `DERIVED_VIEW` 派生数据集并级联同步。
- `GET /api/analytics/history`（`DATA_READ` 看本人，`DATA_ADMIN` 看全租户）：SQL/透视/指标/问数/变换执行历史分页查询。
- `POST /api/chat/query`（`DATA_READ`，扩展）：新增 `session_id?` 与 `mode?: "AUTO"|"DOC"|"DATA"`，支持 `DOC`、`DATA`、`HYBRID`（`[文n]` + `[表]` 混合合成）与 `related_nodes` 业务节点关联返回。

### `xBlueStudio` 多子图本体建模、草案发布、表达式与动态聚合（`/api/xblue/*`，Step 05 & Step 06）

- `GET /api/xblue/subgraphs` / `POST /api/xblue/subgraphs` / `PATCH /api/xblue/subgraphs/{slug}` / `DELETE /api/xblue/subgraphs/{slug}`（`DATA_READ` / `STUDIO_EDIT` / `STUDIO_PUBLISH`）：企业主图（`main`）与部门独立子图工作空间管理。
- `POST /api/xblue/subgraphs/{slug}/merge-preview` / `POST /api/xblue/subgraphs/{slug}/merge`（`STUDIO_EDIT` / `STUDIO_PUBLISH`）：部门子图与主图跨图实体对齐及合并冲突消解（`EXTEND_PROPS` / `KEEP_MAIN` / `OVERWRITE_MAIN`）。
- `GET /api/xblue/schema` / `POST /api/xblue/schema` / `POST /api/xblue/schema/validate`（`DATA_READ` / `STUDIO_PUBLISH` / `STUDIO_EDIT`）：按 `?subgraph=` 查询、直接保存或静态校验模型文档（`SchemaDoc`）。
- `GET /api/xblue/schema/versions` / `GET /api/xblue/schema/versions/{v}` / `GET /api/xblue/schema/versions/{v}/diff/{other}` / `POST /api/xblue/schema/versions/{v}/restore`（`DATA_READ` / `STUDIO_EDIT`）：模型版本历史、结构差异对比与回滚草案生成。
- `GET /api/xblue/drafts` / `POST /api/xblue/drafts` / `POST /api/xblue/drafts/ai` / `GET /api/xblue/drafts/{id}` / `PUT /api/xblue/drafts/{id}` / `POST /api/xblue/drafts/{id}/discard` / `POST /api/xblue/drafts/{id}/publish`（`DATA_READ` / `STUDIO_EDIT` / `STUDIO_PUBLISH`）：模型草案生命周期、AI 自动建图与咨询锁乐观并发发布。
- `GET /api/xblue/mappings` / `POST /api/xblue/mappings/preview` / `POST /api/xblue/mappings/{id}/run` / `GET /api/xblue/mappings/{id}/runs`（`DATA_READ` / `STUDIO_EDIT`）：数据集到图谱映射列表、样本试跑预览、主图全量/增量物化触发与运行记录。
- `GET /api/xblue/objects` / `GET /api/xblue/objects/{type_name}` / `GET /api/xblue/objects/{type_name}/{node_key}`（`DATA_READ`）与 `POST /api/xblue/graphql` / `GET /api/xblue/graphql/sdl`（`DATA_READ`）：主图已发布类型自动暴露的强类型领域对象 REST 与动态 GraphQL Schema。
- `POST /api/xblue/expr/validate`（`STUDIO_EDIT`）：在指定 `NodeType` 作用域下解析并静态类型检查表达式，返回推断类型与带列号的诊断列表。
- `POST /api/xblue/expr/evaluate`（`DATA_READ`）：在指定 `node_id` 当前快照上只读试算表达式，返回值、类型与求值警告（`DIV_BY_ZERO` / `DOMAIN_ERROR`）。
- `POST /api/xblue/aggregate/recompute`（`STUDIO_EDIT`）：入队全量或按 `type_name` 限定的 `GRAPH_AGGREGATE` 重算任务。
- `GET /api/xblue/nodes/{id}/explain/{metric}`（`DATA_READ`）：返回指标来源（`AGG` / `DERIVED` / `SYNC` / `MANUAL` / `ACTION` / `ALGORITHM`）、规则定义、上游输入节点列表与求值警告。
- `GET /api/xblue/knowledge/node-candidates` / `POST /api/xblue/knowledge/node-candidates/{entity_id}/accept`（`STUDIO_EDIT`）：知识库高频未链接实体反哺节点候选列表及确认建点。

### `AxFlow` DAG 引擎、持久化执行与流程版本（`/api/axflow/*`，Step 07）

- `GET /api/axflow/operators`（`DATA_READ`）：返回已注册算子规格清单（`OperatorSpecDto[]`：含 `kind`、`display_name`、`description`、`config_schema` JSON Schema；`AGENT_TASK` 在 Step 09 注册）。
- `GET /api/axflow/flows` / `POST /api/axflow/flows`（`DATA_READ` / `FLOW_EDIT`）：分页查询流程列表（`{items, next_cursor}`）或创建流程草稿（接受 `nodes` / `edges` / `trigger_cfg` / `slug`，兼容历史线性 `steps` 自动转换为链式 DAG；序列化响应不再输出 `steps`）。
- `GET /api/axflow/flows/{id}` / `PUT /api/axflow/flows/{id}` / `DELETE /api/axflow/flows/{id}`（`DATA_READ` / `FLOW_EDIT`）：获取流程详情、保存草稿（返回更新后的 `AxFlowDef` 与静态校验 `issues`，允许保存有问题的草稿）或级联删除流程。
- `POST /api/axflow/flows/{id}/validate`（`FLOW_EDIT`）：对流程当前草稿执行 8 条静态校验规则（节点 ID 与上限、边端点与去重、Kahn 环检测 `CYCLE_DETECTED`、根节点存在、算子注册 `UNSUPPORTED_FORMAT`、算子配置 JSON Schema `SCHEMA_INVALID`、`when` 表达式与 `{{...}}` 祖先引用 `EXPR_INVALID`、触发器合法性），返回 `{valid, issues}`。
- `GET /api/axflow/flows/{id}/versions` / `POST /api/axflow/flows/{id}/versions`（`DATA_READ` / `FLOW_PUBLISH`）：列出已发布版本与当前 `TEST` / `PROD` 部署（`{items, deployments, next_cursor}`），或在草稿零错误时发布新版本（`version = max + 1`）。
- `POST /api/axflow/flows/{id}/deploy`（`TEST` 需 `FLOW_EDIT`，`PROD` 需 `FLOW_PUBLISH`）：将指定已发布版本绑定到 `TEST` 或 `PROD` 环境（支持指向旧版本回退）。
- `POST /api/axflow/flows/{id}/runs`（`FLOW_RUN`）：启动持久化 DAG 运行（`{environment?, version?, payload?, idempotency_key?}` → `202 {run_id, status, existing}`；同一 `(flow_id, idempotency_key)` 幂等返回已有运行）。
- `POST /api/axflow/flows/{id}/run`（`FLOW_RUN`）：兼容入口，等价于 `POST /api/axflow/flows/{id}/runs` 且 `environment = "TEST"`，返回 `{run_id, status, existing}`。
- `GET /api/axflow/flows/{id}/runs` / `GET /api/axflow/runs/{run_id}` / `GET /api/axflow/runs/{run_id}/steps`（`DATA_READ`）：查询流程运行历史分页列表、单次运行详情（含叶子输出 `outputs`）与逐节点尝试日志列表（`{items, next_cursor}`）。
- `POST /api/axflow/runs/{run_id}/cancel` / `POST /api/axflow/runs/{run_id}/retry`（`FLOW_RUN`）：取消运行（置 `CANCELED`、`PENDING` 步骤置 `SKIPPED` 并取消对应 `FLOW_STEP` 任务，执行中算子在检查点退出）或从失败节点以 `attempt + 1` 重新推进。
- `POST /api/axflow/flows/{id}/hook`（`FLOW_PUBLISH`）：创建或轮换流程 Webhook Bearer 秘密（密钥环加密存入 `ds_credentials`，明文秘密在响应中一次性返回）。
- `POST /api/axflow/hooks/{tenant_id}/{slug}`（外部入口，`Authorization: Bearer <secret>`）：经 `ax_resolve_flow_hook` 常量时间校验秘密后，以请求体作为 `trigger` 载荷、`Idempotency-Key` 请求头作为幂等键启动该流程的 `PROD` 部署运行。
- `GET /api/axflow/approvals` / `POST /api/axflow/approvals/{id}/decide`（`DATA_READ` / `APPROVAL_DECIDE`）：查询待审批/历史审批列表（支持 `?status=PENDING&mine=true&run_id=`）或提交 `{decision: "APPROVE"|"REJECT", comment?}`（决定者须属于 `assignee_roles` 或 `assignee_users`，否则 `403 PERMISSION_DENIED`；单事务写 `ax_approvals`、`approval.decided` 外发事件与 `APPROVAL_DECIDE` 审计）。
- `GET /api/axflow/actions`（`DATA_READ`）：查询动作回写流水 `ax_action_log`（支持 `?run_id=&status=`，`TEST` 环境或 `dry_run=true` 记录为 `DRY_RUN`）。

### 规则引擎、告警中心与通知渠道（`/api/axflow/{rules,alerts,channels,silences}*`，Step 08）

- `GET /api/axflow/rules` / `POST /api/axflow/rules`（`DATA_READ` / `RULE_EDIT`）：分页查询或创建规则定义（支持 `scope_kind: METRIC|DATASET_QUALITY|EVENT`、`THRESHOLD|CHANGE_RATE|ABSENCE|ANOMALY_ZSCORE|EXPR|QUALITY_FAILED|EVENT_MATCH` 7 种条件、冷却/恢复/升级分钟数、通知渠道与 `run_flow_id` 联动流程）。
- `GET /api/axflow/rules/{id}` / `PUT /api/axflow/rules/{id}` / `DELETE /api/axflow/rules/{id}`（`DATA_READ` / `RULE_EDIT`）：读取、更新或删除规则（停用规则时自动将活动告警置为 `RESOLVED(RULE_DISABLED)`）。
- `POST /api/axflow/rules/{id}/dry-run` / `POST /api/axflow/rules/dry-run`（`RULE_EDIT`）：对已保存规则或草稿规则定义执行一次只读试判定（不落库、不发通知），返回 `{rule_id, evaluated_objects, matched_count, would_trigger}`。
- `GET /api/axflow/alerts` / `GET /api/axflow/alerts/{id}` / `GET /api/axflow/alerts/{id}/events`（`DATA_READ`）：按 `?status=&severity=&rule_id=&node_id=&type_name=` 分页查询告警、查询单条告警详情（含关联 `deliveries`）及时间线事件流 `ax_alert_events`。
- `POST /api/axflow/alerts/{id}/ack` / `POST /api/axflow/alerts/{id}/resolve` / `POST /api/axflow/alerts/ack-batch`（`ALERT_ACK`）：单条确认（`OPEN -> ACK`）、手动恢复（`RESOLVED(MANUAL)` 并触发 `alert.resolved`）或批量确认（`{alert_ids, comment?}`）。
- `GET /api/axflow/channels` / `POST /api/axflow/channels` / `PUT /api/axflow/channels/{id}` / `DELETE /api/axflow/channels/{id}`（`DATA_READ` / `RULE_EDIT`）：管理 `WEBHOOK`、`EMAIL`、`WECOM`、`DINGTALK`、`FEISHU` 五类通知渠道；秘密字段经密钥环加密写入 `ds_credentials`（仅回显 `secret_fields_set`），设置 `allow_private_network = true` 需具备 `GUARD_ADMIN`。
- `POST /api/axflow/channels/{id}/test`（`RULE_EDIT`）：经 `NetGuard` 校验后同步发送一条 `event = TEST` 测试消息并返回 `{ok, delivery}`。
- `GET /api/axflow/silences` / `POST /api/axflow/silences` / `DELETE /api/axflow/silences/{id}`（`DATA_READ` / `ALERT_ACK`）：查询、创建（`ends_at > starts_at`）或删除告警静默窗口 `ax_alert_silences`。

### `AxAgent` 个人访问令牌、多模型路由、能力发布、MCP 与多轮智能体（`/api/auth/tokens*`、`/api/axagent/*`、`/api/published/*`、`/api/mcp`，Step 09）

- `GET /api/auth/tokens` / `POST /api/auth/tokens` / `DELETE /api/auth/tokens/{id}`（已认证用户）：列出当前用户个人访问令牌（不含明文）、签发 `axp_<prefix8>_<secret32>`（`scopes` 须为当前用户权限子集，一次性返回明文并审计 `TOKEN_ISSUE`）或吊销令牌（审计 `TOKEN_REVOKE`）。
- `GET /api/axagent/providers` / `POST /api/axagent/providers` / `PUT /api/axagent/providers/{id}` / `DELETE /api/axagent/providers/{id}` / `POST /api/axagent/providers/{id}/test`（`AGENT_EDIT`；`allow_private_network=true` 需 `GUARD_ADMIN`）：多模型提供方注册、密钥环加密存储、优先级与熔断配置及连通性测试。
- `GET /api/axagent/usage?from=&to=`（`AGENT_EDIT`）：按提供方、日期与用途查询请求量、错误数与 Token 用量统计（`ax_llm_usage`）。
- `GET /api/axagent/endpoints` / `POST /api/axagent/endpoints` / `PUT /api/axagent/endpoints/{id}` / `DELETE /api/axagent/endpoints/{id}` / `POST /api/axagent/endpoints/{id}/promote`（`DATA_READ` / `STUDIO_PUBLISH`；`kind=FLOW` 需 `FLOW_PUBLISH`）：管理五类发布端点（`SAVED_QUERY`、`METRIC`、`OBJECT_TYPE`、`FLOW`、`AGENT`），自动推导 Draft 2020-12 `input_schema`，并在提升至 `PROD` 时固定 `target_version` 与 `target_snapshot`。
- `GET /api/axagent/endpoints/openapi.json?environment=PROD` / `GET /api/axagent/endpoints/{id}/skill.json`（`DATA_READ`）：导出 OpenAPI 3.1 规范文档与外部智能体 Skill 清单。
- `POST /api/axagent/apps/{endpoint_id}/run`（端点 `required_scope`）：查询即应用内部执行入口，以当前登录主体直接调用目标能力。
- `POST /api/published/{env}/{tenant_id}/{slug}` / `GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}`（`Authorization: Bearer axp_...` 个人访问令牌）：校验租户与 `required_scope`、`(endpoint_id, token_id)` 速率限制、JSON Schema 入参、`moka` TTL 缓存后执行能力并写审计日志。
- `POST /api/mcp` / `GET /api/mcp` / `DELETE /api/mcp`（`Authorization: Bearer axp_...` 个人访问令牌）：Streamable HTTP MCP Server（`Mcp-Session-Id` 会话管理），动态暴露 `PROD` 且 `expose_mcp=true` 的发布端点与内置只读工具（`search_knowledge`、`ask_data`、`list_datasets`、`describe_dataset`、`query_objects`、`graph_neighbors`、`get_node`）及 `axpoint://datasets/*`、`axpoint://objects/*` 资源，每次 `tools/call` 写 `MCP_CALL` 审计。
- `GET /api/axagent/mcp-clients` / `POST /api/axagent/mcp-clients` / `DELETE /api/axagent/mcp-clients/{id}` / `POST /api/axagent/mcp-clients/{id}/sync`（`AGENT_EDIT`）：外部 Streamable HTTP MCP 服务登记（经 `NetGuard` 校验）与 `tools/list` 同步缓存。
- `GET /api/axagent/agents` / `POST /api/axagent/agents` / `GET /api/axagent/agents/{id}` / `PUT /api/axagent/agents/{id}` / `DELETE /api/axagent/agents/{id}` / `POST /api/axagent/agents/{id}/publish`（`DATA_READ` / `AGENT_EDIT`）：智能体草稿编辑、多智能体嵌套深度校验（`<= 2`）与不可变版本发布（`ax_agent_versions`）。
- `POST /api/axagent/agents/{id}/chat`（`AGENT_RUN`；`draft=true` 需 `AGENT_EDIT`）：智能体多轮工具循环对话（支持 `Accept: text/event-stream` 流式 `token|tool_call|tool_result|approval_required|final|error` 事件与 `Accept: application/json` 同步返回，写 `ax_messages` 与 `AGENT_CHAT` 审计）。
- `GET /api/axagent/conversations` / `GET /api/axagent/conversations/{id}/messages` / `DELETE /api/axagent/conversations/{id}`（`AGENT_RUN`，仅本人会话）：工作台多轮会话与消息流管理。
- `POST /api/axagent/agents/{id}/embeds` / `GET /api/axagent/agents/{id}/embeds` / `DELETE /api/axagent/embeds/{embed_id}`（`VIEW_SHARE`）与 `POST /api/axagent/embed/{token}/chat`（公开嵌入令牌 `axe_...`）：按权限子集与 `allowed_origins` 签发、查询、吊销外部嵌入链接及按 `(embed_id, client_ip)` 限流的公开对话入口。

### `AxSim` 写时复制沙盘、因果推演、对比问数与决策下发（`/api/axsim/*`，Step 10）

- `GET /api/axsim/sandboxes` / `POST /api/axsim/sandboxes` / `GET /api/axsim/sandboxes/{id}` / `DELETE /api/axsim/sandboxes/{id}`（`DATA_READ` / `SIM_RUN`）：按范围（`scope: {root_node_ids, depth<=6, direction, type_names, link_types, max_nodes<=50000}`）从生产业务图克隆基线快照到 `ax_sim_base_*`，管理沙盘生命周期（`CLONING | READY | ARCHIVED`）。
- `GET /api/axsim/sandboxes/{id}/graph`（`DATA_READ`）：返回 `base` 与 `overlay` 合并后的沙盘节点与关系视图（含 `is_modified`、`is_new`、`overlay_hash`）。
- 覆盖层编辑（`SIM_RUN`）：`PATCH /api/axsim/sandboxes/{id}/nodes/{node_id}`、`POST /api/axsim/sandboxes/{id}/nodes`、`DELETE /api/axsim/sandboxes/{id}/nodes/{node_id}`、`POST /api/axsim/sandboxes/{id}/links`、`PATCH /api/axsim/sandboxes/{id}/links/{link_id}`、`DELETE /api/axsim/sandboxes/{id}/links/{link_id}`、`POST /api/axsim/sandboxes/{id}/reset`、`POST /api/axsim/sandboxes/{id}/refresh-base`（保留覆盖层并列出孤立 ID）；所有操作不修改生产表。
- `POST /api/axsim/sandboxes/{id}/simulate`（`SIM_RUN`）：快速因果推演，支持 `{actions, params?}`，支持 `RATIO | ADD | ABSOLUTE` 及 `delta_ratio` 简写；目标节点缺失指标且模式为 `RATIO`/`ADD` 返回 `400 INVALID_ARGUMENT`。
- `GET /api/axsim/sandboxes/{id}/scenarios` / `POST /api/axsim/sandboxes/{id}/scenarios` / `GET /api/axsim/scenarios/{id}` / `POST /api/axsim/scenarios/{id}/rerun` / `DELETE /api/axsim/scenarios/{id}`（`DATA_READ` / `SIM_RUN`）：创建并求解稳态（SCC + 阻尼雅可比迭代）或分步（`t = 0..=horizon_steps`）场景，可选 Monte-Carlo 不确定性抽样（`P10`/`P90`），记录 `overlay_hash` 并在覆盖层变化后标为过期。
- `GET /api/axsim/scenarios/{id}/impacts?type_name=&metric=&limit=&cursor=` / `GET /api/axsim/scenarios/{id}/checkpoints`（`DATA_READ`）：分页查询受影响节点（含 `before_val`、`after_val`、`delta`、`delta_ratio`、`p10`、`p90`、`hop_distance`、`top_path`、`series`）及逐步回放检查点。
- `GET /api/axsim/scenarios/compare?ids=&metrics=&type_name=&briefing=`（`DATA_READ`）：同沙盘内 2～5 个场景 KPI 与节点级并排对比；`briefing=true` 返回经数值校验的 AI 研判 `{summary, recommended_scenario_id, reasons[], risks[]}`。
- `POST /api/axsim/ask`（`SIM_RUN`）：推演问数，自动识别 `WHAT_IF`（大模型解析干预 + 节点检索消歧 + 创建运行场景）或 `QUERY_RESULT`（内存 DuckDB `impacts` 表 NL2SQL），写 `ASK_DATA` 审计。
- `POST /api/axsim/scenarios/{id}/decisions` / `GET /api/axsim/decisions/{id}` / `POST /api/axsim/decisions/{id}/rebase`（`SIM_RUN`，审批需 `SIM_APPLY`）：创建下发提案与 `ax_approvals(source_kind=SIM_APPLY)` 审批门；通过后 `SELECT ... FOR UPDATE` 校验生产基线冲突，无冲突则原子写 `xb_nodes.metrics`、`xb_metric_series(ACTION)`、`ax_action_log`、`ax_outbox(sim.applied, node.metrics_changed)`、`sec_audit_logs(SIM_APPLY)` 并按需触发 `PROD` 流程；存在冲突则标为 `CONFLICT` 并支持 `/rebase`。

### `xBlueView` 孪生驾驶舱、AI 看板生成、空间绑定与动作回写（`/api/xblueview/*`，Step 11）

- `GET /api/xblueview/dashboards` / `POST /api/xblueview/dashboards` / `GET /api/xblueview/dashboards/{id}` / `PUT /api/xblueview/dashboards/{id}` / `DELETE /api/xblueview/dashboards/{id}` / `GET /api/xblueview/dashboards/by-slug/{slug}`（`DATA_READ` / `VIEW_EDIT`）：驾驶舱增删改查；非 `VIEW_EDIT` 主体仅可见 `PUBLISHED` 驾驶舱，slug 严格校验 `^[a-z0-9][a-z0-9-]{0,62}$`。
- `POST /api/xblueview/dashboards/ai-generate`（`VIEW_EDIT`）：收集租户主图 `SchemaDoc`、语义指标、数据集及空间资产上下文，经 `LlmRouter(Purpose::Extract)` 生成 24 列栅格驾驶舱草案（校验并自动自修一次），落库为 `DRAFT`。
- `POST /api/xblueview/dashboards/{id}/publish` / `POST /api/xblueview/dashboards/{id}/duplicate` / `PUT /api/xblueview/dashboards/{id}/layout`（`VIEW_EDIT`）：发布版本（`version + 1`、`status=PUBLISHED`）、复制为新草稿、批量更新组件栅格坐标 `{x,y,w,h}`。
- `POST /api/xblueview/dashboards/{id}/widgets` / `PUT /api/xblueview/widgets/{id}` / `DELETE /api/xblueview/widgets/{id}`（`VIEW_EDIT`）：维护 20 类组件（`KPI_CARD`、`TABLE`、`PIVOT_TABLE`、各类图表、`GRAPH_TOPOLOGY`、`MAP`、`FLOOR_PLAN`、`SCENE_3D`、`ALERT_LIST`、`SIM_COMPARE`、`ACTION_BUTTON`、`ACTION_FORM`、`MARKDOWN_NOTE`）并在保存时计算 GIN 依赖 `deps`。
- `POST /api/xblueview/widgets/{id}/data` / `POST /api/xblueview/widgets/preview`（`DATA_READ`）：统一组件执行入口，支持 `SAVED_QUERY` / `METRIC` / `SQL` / `NODE_METRIC` / `PIVOT` / `GRAPH` / `ALERTS` / `SIM_SCENARIOS` / `NODES_GEO` / `FLOOR_PLAN` / `SCENE_3D` / `ACTION` / `STATIC`，参数化注入全局筛选与下钻包装查询，并按 `(tenant_id, widget_id, hash(filters, drill_path, principal_fingerprint))` 在 `moka` 缓存（TTL 600s）。
- `POST /api/xblueview/widgets/{id}/invoke`（`FLOW_RUN`；禁止分享令牌访问）：按组件 `input_schema` 校验入参；`RUN_FLOW` 触发目标流程 `PROD` 部署，`WRITE_NODE` 校验主体部门与 `NodeTypeSpec` 后在单事务内更新 `xb_nodes`、`xb_metric_series(ACTION)`、`ax_action_log(source_kind='VIEW')` 并外发 `node.upserted`/`node.metrics_changed`。
- `GET /api/xblueview/dashboards/{id}/stream`（`DATA_READ`，SSE `text/event-stream`）：通过 `PgListener('ax_view_events')` + 进程内广播通道向同租户驾驶舱实时推送 `event: invalidate` 与 `event: alert`（15s `KeepAlive`）。
- `GET /api/xblueview/assets` / `POST /api/xblueview/assets` / `GET /api/xblueview/assets/{id}/content` / `DELETE /api/xblueview/assets/{id}`（`DATA_READ` / `VIEW_EDIT`）：管理 `FLOOR_PLAN_IMAGE`（含 SVG 白名单清洗，≤20 MB）、`GEOJSON`（校验 `FeatureCollection`，≤20 MB）、`ICON`（≤20 MB）与 `MODEL_3D`（校验 `.glb` 12 字节头魔数、版本 `2` 与长度，≤50 MB）。
- `GET /api/xblueview/assets/{id}/placements` / `PUT /api/xblueview/assets/{id}/placements` 与 `GET /api/xblueview/assets/{id}/bindings3d` / `PUT /api/xblueview/assets/{id}/bindings3d`（`DATA_READ` / `VIEW_EDIT`）：读写 2D 平面图比例坐标与 3D `glTF` 网格/桩点节点绑定。

---

## 2. AxParser Sidecar（`axpoint-parser`，端口 `8001`，仅内部网络）

除 `GET /health` 外，所有请求携带 `X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN` 与 `X-AxPoint-Tenant`（可选 `X-AxPoint-Timezone`、`traceparent`），否则返回 `401`。

### `GET /health`
返回解析引擎与模型目录（`${AXPOINT_MODEL_PATH:-/var/lib/axpoint/models}`）就绪状态。

### `POST /parse`
`multipart/form-data`（字段 `file`），仅返回文本轨轻量结果 `ParseResponse {title, markdown, engine}`（不写 Parquet 暂存文件）。

### `POST /parse/unified`（Step 01）
`multipart/form-data`（`file`、`log_format?`、`json_explode?`、`ocr?`），返回 `UnifiedParseResponse {parse_id, title, source_format, engine, sha256, markdown, tables, warnings}`，表格轨 Parquet 暂存文件写入 `${AXPOINT_DATA_ROOT}/tenants/<tenant_id>/staging/<parse_id>/`。

### 连接器与 DuckLake 内部接口（Step 02 & Step 04）

- `GET /connectors/types`：返回 32 个 `ConnectorSpecModel[]`。
- `POST /connectors/test`：测试连接并返回 `TestResult {ok, message, latency_ms}`。
- `POST /connectors/discover`：发现远端对象并返回 `DiscoveredObject[]`。
- `POST /connectors/preview`：抽样预览远端对象（PII 样例脱敏）。
- `POST /connectors/sync`：按 `SNAPSHOT` / `APPEND` / `UPSERT` 同步源数据入 DuckLake，执行质量规则与 Schema 演进，返回 `SyncResult`。
- `POST /lake/commit_staging`：把 `/parse/unified` 生成的暂存 Parquet 原子提交入 DuckLake。
- `POST /lake/preview` / `GET /lake/snapshots` / `POST /lake/maintenance` / `POST /lake/drop_table`：湖表快照预览、历史快照列表、小文件合并与快照清理、删表。
- `POST /lake/materialize`（Step 04）：经 `sqlglot` 二次校验 SQL 仅引用 `lake."tenant_<id>".*` 且 `enable_external_access=false` 后，在写连接事务内执行 `CREATE OR REPLACE TABLE lake."tenant_<id>".<t> AS <sql>` 并返回 `SyncResult`。
