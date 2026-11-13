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

---

## 2. AxParser Sidecar（`axpoint-parser`，端口 `8001`，仅内部网络）

除 `GET /health` 外，所有请求携带 `X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN` 与 `X-AxPoint-Tenant`（可选 `X-AxPoint-Timezone`、`traceparent`），否则返回 `401`。

### `GET /health`
返回解析引擎与模型目录（`${AXPOINT_MODEL_PATH:-/var/lib/axpoint/models}`）就绪状态。

### `POST /parse`
`multipart/form-data`（字段 `file`），仅返回文本轨轻量结果 `ParseResponse {title, markdown, engine}`（不写 Parquet 暂存文件）。

### `POST /parse/unified`（Step 01）
`multipart/form-data`（`file`、`log_format?`、`json_explode?`、`ocr?`），返回 `UnifiedParseResponse {parse_id, title, source_format, engine, sha256, markdown, tables, warnings}`，表格轨 Parquet 暂存文件写入 `${AXPOINT_DATA_ROOT}/tenants/<tenant_id>/staging/<parse_id>/`。

### 连接器与 DuckLake 内部接口（Step 02）

- `GET /connectors/types`：返回 32 个 `ConnectorSpecModel[]`。
- `POST /connectors/test`：测试连接并返回 `TestResult {ok, message, latency_ms}`。
- `POST /connectors/discover`：发现远端对象并返回 `DiscoveredObject[]`。
- `POST /connectors/preview`：抽样预览远端对象（PII 样例脱敏）。
- `POST /connectors/sync`：按 `SNAPSHOT` / `APPEND` / `UPSERT` 同步源数据入 DuckLake，执行质量规则与 Schema 演进，返回 `SyncResult`。
- `POST /lake/commit_staging`：把 `/parse/unified` 生成的暂存 Parquet 原子提交入 DuckLake。
- `POST /lake/preview` / `GET /lake/snapshots` / `POST /lake/maintenance` / `POST /lake/drop_table`：湖表快照预览、历史快照列表、小文件合并与快照清理、删表。
