# xBlueNode / AxPoint 接口契约 (API Contract)

## 1. AxPoint Core (`axpoint-core`, 默认端口 `9995`)

### `GET /api/health`
健康检查与当前 Embedding 维度探测状态。

### `GET /api/stats?tenant_id=default`
返回 `Doc / Para / Entity / Edge / Embedding` 实时计数及已存储的向量维度列表 `embedding_dims`。

### `POST /api/documents/ingest`
直接提交 Markdown/纯文本构建三级超图：
```json
{
  "tenant_id": "default",
  "title": "文档标题",
  "source_name": "doc.md",
  "content": "正文内容..."
}
```

### `POST /api/documents/upload`
`multipart/form-data` 上传文件（字段 `tenant_id`、`file`），由 `axpoint-core` 转发至 `axpoint-parser` 解析为 Markdown 后自动入库。

### `POST /api/chat/query`
并行执行五路召回（Vector / BM25 / EntityLink / Context / MultiHop）+ RRF 融合 + LLM 合成回答：
```json
{
  "tenant_id": "default",
  "query": "什么是动态超图架构？",
  "top_k": 6
}
```

### `GET /api/graph/topology?tenant_id=default`
返回前端 `xBlueGraph` 拓扑浏览器所需的 `nodes` 与 `edges`。

### `GET /api/xblue/schema?tenant_id=default` & `POST /api/xblue/schema`
获取或发布 `xBlueStudio` 业务节点类型（`BlueNodeTypeDef`）与因果关系定义（`BlueEdgeTypeDef`）。

### `GET /api/xblue/nodes?tenant_id=default` & `POST /api/xblue/nodes`
查询或创建/更新运行时业务节点（`BlueNode`）与关联连线（`BlueLink`）。

### `GET /api/axflow/flows?tenant_id=default` & `POST /api/axflow/flows`
查询或创建 `AxFlow` 智能编排流水线（`INGEST` → `TRANSFORM` → `NODE_MAP` → `ACTION`）。

### `POST /api/axflow/flows/{id}/run?tenant_id=default`
触发执行指定 `AxFlow` 流水线并返回执行轨迹日志。

### `GET /api/axsim/sandboxes?tenant_id=default` & `POST /api/axsim/sandboxes`
列出或基于写时复制（CoW）克隆生产节点快照创建隔离 `AxSim` 推演沙盘。

### `POST /api/axsim/sandboxes/{id}/simulate?tenant_id=default`
在隔离 `AxSim` 沙盘中施加 What-If 干预动作（`target_node_id`、`metric_key`、`delta_ratio`），返回多跳因果传导影响明细。

---

## 2. AxParser Sidecar (`axpoint-parser`, 默认端口 `8001`)

### `GET /health`
返回解析引擎与 `/home/antigravity/models/easyocr` 模型就绪状态。

### `POST /parse`
`multipart/form-data`（字段 `file`），返回：
```json
{
  "title": "文档标题",
  "markdown": "解析后的结构化 Markdown",
  "engine": "docling-easyocr | pypdf-fallback | plain-utf8"
}
```
