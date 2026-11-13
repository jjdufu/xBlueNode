# xBlueNode / AxPoint 接口契约（API Contract）

本文件记录**已合入 `main` 的接口**，每个 Step 的契约 PR 把该 Step 的接口条目写入对应小节。全部接口（含各 Step 计划交付的接口）的方法、路径、权限与定义位置见 [blueprint/A4-API-CONTRACT-INDEX.md](./blueprint/A4-API-CONTRACT-INDEX.md)；前缀、分页、错误码、序列化约定见 [blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md](./blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 0. 通用约定

- **鉴权**：除 `GET /api/health` 与 Step 13 §13.2 列出的外部入口外，所有 `/api/*` 请求携带 `Authorization: Bearer <token>`。Step 00 起令牌为开发静态令牌 `AXPOINT_DEV_TOKEN`；Step 09 起支持个人访问令牌（`axp_` 前缀）；Step 13 起支持 EdDSA 访问令牌（有效期 900 秒）与刷新令牌。
- **租户**：租户只取自令牌对应的主体；请求中出现 `tenant_id` 查询参数或请求体字段时返回 `400 INVALID_ARGUMENT`。外部入口的租户取自路径或令牌（A1 §2 规则 2）。
- **错误体**：`{"error_code": "...", "message": "...", "trace_id": "..."}`，状态码与 `error_code` 对照见 A1 §5；跨租户访问一律返回 `404`。
- **分页**：`?limit=50&cursor=<opaque>`，响应 `{items: [...], next_cursor: string | null}`，`limit` 范围 `1..=500`。
- **序列化**：JSON 字段 `snake_case`，时间为 RFC 3339 UTC，ID 为 UUID 字符串（超图表沿用 `BIGINT`）。

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
权限 `DATA_READ` / `STUDIO_PUBLISH`。读取或写入业务节点类型（`BlueNodeTypeDef`）与关系类型（`BlueEdgeTypeDef`）。Step 05 起同一路径写入经过校验与版本记录，增量编辑走草案 → 发布流程（`/api/xblue/drafts/*`）。

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

---

## 2. AxParser Sidecar（`axpoint-parser`，端口 `8001`，仅内部网络）

除 `GET /health` 外，所有请求携带 `X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN` 与 `X-AxPoint-Tenant`，否则返回 `401`。

### `GET /health`
返回解析引擎与模型目录（`${AXPOINT_MODEL_PATH:-/var/lib/axpoint/models}`）就绪状态。

### `POST /parse`
`multipart/form-data`（字段 `file`），返回：

```json
{ "title": "文档标题", "markdown": "解析后的结构化 Markdown", "engine": "docling-easyocr | pypdf-fallback | plain-utf8" }
```

### `POST /parse/unified`
Step 01 交付的双轨解析接口（文本轨 Markdown + 表格轨数据表），请求与响应定义见 [blueprint/02-STEP01-AXPARSER-FILE-PARSING.md](./blueprint/02-STEP01-AXPARSER-FILE-PARSING.md) §01.2。
