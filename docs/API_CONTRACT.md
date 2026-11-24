# xBlueNode / AxPoint 接口契约（API Contract）

本文件记录**已合入 `main` 的接口**，每个 Step 的契约 PR 把该 Step 的接口条目写入对应小节。全部接口的方法、路径、权限与定义位置见 [standards/A4-API-CONTRACT-INDEX.md](./standards/A4-API-CONTRACT-INDEX.md)；前缀、分页、错误码、序列化约定见 [standards/A1-CONVENTIONS-ENUMS-ROUTES.md](./standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

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

### `GET /api/documents` / `DELETE /api/documents/{id}`
权限 `DATA_READ` / `DATA_SYNC`。分页查询已入库超图知识文档列表（含段落数、实体数、来源与摘要，支持 `?q=&limit=&cursor=`）及按 ID 级联删除文档与关联段落/向量/超图边。

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

- `GET /api/xblue/nodes`（`DATA_READ`）：支持 `?subgraph=&type_name=&q=&bbox=minLng,minLat,maxLng,maxLat&geo_op=BBOX|GEO_WITHIN_POLYGON|GEO_DWITHIN_METERS&polygon=<GeoJSON>&center_lng=&center_lat=&radius_meters=&limit=&cursor=` 两级空间几何过滤（PostgreSQL B-Tree 外接矩形粗筛 + `geo` 多边形包含 / 球面 Haversine 精确判定，非法几何返回 `400 INVALID_GEOMETRY`），返回 `{items, nodes, links, next_cursor}`（Step 03 / Step 28）。
- `POST /api/xblue/geo/convex-hull {subgraph?, type_name?, node_ids?, points?}`（`DATA_READ` → `200 ConvexHullResponse {hull, point_count, area_sq_deg, bbox?}`）：对受行级策略治理的节点坐标集或内联 WGS-84 点集执行纯 Rust 凸包求解并返回 `GeoShapeDto`（Step 28）。
- `POST /api/xblue/nodes`（`STUDIO_EDIT`）：幂等 upsert 业务节点（支持 `properties`、`metrics_merge: MERGE|REPLACE`、`geo`、`geo_shape`、乐观锁 `if_row_version`、关联边创建），入队 `EMBED_NODES` 并写 `ax_outbox`（Step 03 / Step 28）。
- `POST /api/xblue/nodes/bulk`（`STUDIO_EDIT`）：单批最多 5 000 个节点 + 20 000 条关系（`UNNEST` 批量写入，端点支持 `{type_name, node_key}` 解析）。
- `GET /api/xblue/nodes/{id}` / `PATCH /api/xblue/nodes/{id}` / `DELETE /api/xblue/nodes/{id}`（`DATA_READ` / `STUDIO_EDIT`）：单节点查询、增量更新与级联删除（连同 `xb_links` 与 `xb_metric_series`）。
- `GET /api/xblue/nodes/search?q=&type_name=&k=20`（`DATA_READ`）：PostgreSQL FTS + `pgvector` HNSW 双通道 RRF（`k=60`）混合检索。
- `GET /api/xblue/nodes/{id}/documents`（`DATA_READ`）：经 `xb_entity_links` 与 `MENTIONS` 超边查询提及该节点的文档与段落摘录。
- `GET /api/xblue/links` / `POST /api/xblue/links` / `DELETE /api/xblue/links/{id}`（`DATA_READ` / `STUDIO_EDIT`）：业务关系列表、幂等 upsert（唯一键 `(tenant_id, link_type, src_node_id, dst_node_id)`）与删除。
- `POST /api/xblue/metrics/write`（`DATA_SYNC`）：批量写入时序原始点（最多 50 000 点），同步更新 `xb_nodes.metrics` 最新值与 `xb_rollup_dirty` 迟到桶，并入队 `ROLLUP`。
- `GET /api/xblue/nodes/{id}/metrics/{metric}?from=&to=&grain=&agg=`（`DATA_READ`）：自动或显式粒度时序查询（超过 5 000 点自动升粒度）。
- `POST /api/xblue/metrics/window` / `GET /api/xblue/metrics/catalog`（`DATA_READ`）：滚动/滑动窗口聚合查询与租户指标目录。
- `GET /api/xblue/graph/neighbors` / `GET /api/xblue/graph/path` / `GET /api/xblue/graph/subgraph`（`DATA_READ`）：防环递归 CTE k 跳邻居、双向 BFS / Dijkstra 最短路径与画布子图；`GET /api/xblue/graph/subgraph` 支持 `?type_names=&link_types=&subgraph=&src_type=&edge_type=&dst_type=&limit=` 三元图模式 `(src:NodeType)-[r:EdgeType]->(dst:NodeType)` 诱导子图筛选（Step 03 / Step 20）。
- `POST /api/xblue/graph/algorithms/{algo}` / `GET /api/xblue/graph/algorithms/{algo}/results`（`STUDIO_EDIT` / `DATA_READ`）：`PAGERANK`、`DEGREE`、`LABEL_PROPAGATION`、`CONNECTED_COMPONENTS` 图算法运行与分页结果读取。
- `POST /api/xblue/entities/resolve` / `GET /api/xblue/entities/candidates` / `POST /api/xblue/entities/links/{entity_id}/{node_id}/confirm|reject`（`STUDIO_EDIT`）：超图实体与业务节点归一化自动对齐与人工确认/拒绝。
- `POST /api/axops/jobs/rollup-now`（`OPS_ADMIN`）：为当前租户立即入队一次 `ROLLUP` 任务并返回 `{job_id}`。

### `AxAnalytics` OLAP、透视制表、指标语义层与智能问数（`/api/analytics/*` 与 `/api/chat/query` 扩展，Step 04）

- `GET /api/analytics/catalog`（`DATA_READ`）：返回当前租户在湖中的数据集与列目录（含 PII 与业务术语）、`ax.*` 平台联邦视图及 `live_*` 直连对象树。
- `POST /api/analytics/sql/run` / `POST /api/analytics/sql/explain`（`DATA_READ`）：经 `sql_guard` AST 白名单与租户表改写后执行或解释只读 `DuckDB` SQL；支持 `strict_ontology_join: true` 按已发布 `MappingLinkSpec` 校验跨表 `JOIN`（未命中法定边返回 `400 JOIN_PATH_UNGROUNDED`）；默认对所有主体动态脱敏 PII 列（除非 `reveal_pii=true` 且具备 `PII_RAW_READ`），并记录 `an_query_history` 与 `QUERY_SQL` 审计（Step 04 / Step 16）。
- `POST /api/analytics/axql/compile {axql, params?, subgraph_slug?}`（`DATA_READ` → `200 AxqlCompilePlan`）与 `POST /api/analytics/axql/run {axql, params?, max_rows?, timeout_ms?, reveal_pii?, subgraph_slug?}`（`DATA_READ` → `200 AxqlQueryResponse`）：将 `FROM OBJECT(<Type>)`（三层侧面预聚合免膨胀折叠）、`FROM MATCH (a)-[:REL]->(b)`、`SUM_BY_ENTITY(expr BY alias)` / `AVG_BY_ENTITY` 与 `MATCH_KNOWLEDGE($q, top_k=N)` 确定性编译为受 `sql_guard` 与 PII 掩码保护的 `DuckDB` SQL；引用不存在对象/属性返回 `400 AXQL_UNMAPPED_SYMBOL`，跨无关联对象返回 `400 JOIN_PATH_UNGROUNDED`（Step 27）。
- `POST /api/analytics/pivot`（`DATA_READ`）：拖拽式透视制表编译与交叉矩阵化（支持 `GROUPING SETS` 小计/总计、列组合频次上限与非可加聚合截断提示）。
- `GET /api/analytics/metrics` / `POST /api/analytics/metrics` / `GET /api/analytics/metrics/{name}` / `PATCH /api/analytics/metrics/{name}` / `DELETE /api/analytics/metrics/{name}`（`DATA_READ` / `STUDIO_EDIT`，`certified=true` 需 `STUDIO_PUBLISH`）：指标语义层管理、聚合表达式校验、破坏性 Schema 检测（`broken=true`）与血缘记录。
- `POST /api/analytics/metrics/{name}/query` / `POST /api/analytics/metrics/query-multi`（`DATA_READ`）：单指标或多指标按维度、时间粒度与同比/环比（`PREVIOUS_PERIOD` / `SAME_PERIOD_LAST_YEAR`）编译查询。
- `GET /api/analytics/saved-queries` / `POST /api/analytics/saved-queries` / `GET /api/analytics/saved-queries/{id}` / `PATCH /api/analytics/saved-queries/{id}` / `DELETE /api/analytics/saved-queries/{id}` / `POST /api/analytics/saved-queries/{id}/run`（`DATA_READ` / `VIEW_EDIT`）：参数化保存查询管理与运行。
- `POST /api/analytics/ask` / `POST /api/analytics/ask/{turn_id}/feedback` / `GET /api/analytics/ask/sessions/{session_id}`（`DATA_READ`）：智能问数管线（第 0 阶段认证指标与领域对象槽位确定性编译、FTS+向量模式链接、`sql_guard` + 法定 `JOIN` 守卫 + `EXPLAIN` 自修复、数值逐字核验摘要、零依据确定性拒答、图表建议、多轮会话与少样本反馈），`context` 支持 `{dataset_ids, doc_ids, doc_title_prefix?, subgraph_slug?, endpoint_slugs, filters, drill_path, dashboard_id?}` 显式作用域硬隔离（越权目标返回 `403`），返回接地元数据 `{grounding_mode, verified_numbers, verified_citations, abstained, slot_payload?}`（Step 04 / Step 16 / Step 20）。
- `POST /api/analytics/grounding-eval/run`（`GUARD_ADMIN` 或 `OPS_ADMIN`）/ `GET /api/analytics/grounding-eval/runs?limit=20`（`DATA_READ`）：执行内置 50 例云/端双轨（`CLOUD` + `LOCAL_7B`）反幻觉对抗基准并持久化或查询历史评测记录 `an_grounding_eval_runs`（Step 16）。
- `POST /api/analytics/transform/draft` / `PATCH /api/analytics/transform/{id}` / `POST /api/analytics/transform/{id}/apply` / `POST /api/analytics/transform/{id}/discard`（`DATA_SYNC`）：自然语言 → `SELECT` 变换草案 → 100 行脱敏预览 → 确认物化为 `DERIVED_VIEW` 派生数据集并级联同步。
- `GET /api/analytics/history`（`DATA_READ` 看本人，`DATA_ADMIN` 看全租户）：SQL/透视/指标/问数/变换执行历史分页查询。
- `POST /api/chat/query`（`DATA_READ`）：支持 `session_id?`、`mode?: "AUTO"|"DOC"|"DATA"` 与会话级显式作用域 `scope?: ChatQueryScope {dataset_ids, doc_ids, doc_title_prefix?, subgraph_slug?, endpoint_slugs}`（五路超图召回在 SQL 层按 `doc_ids` 与转义 `doc_title_prefix` 过滤，`TenantCatalog` 与认证指标按 `dataset_ids` 硬裁剪，越权目标返回 `403`），覆盖 `DOC`（`[文N]` 段落引用越界剥离与零召回 `ABSTAINED` 拒答）、`DATA`、`HYBRID`（`[文N]` + `[表]` 双重核验合成）与 `related_nodes` 业务节点关联返回，附带 `{grounding_mode, verified_numbers, verified_citations, abstained, slot_payload?, applied_scope?}`（Step 04 / Step 16 / Step 20）。

### `xBlueStudio` 多子图本体建模、草案发布、表达式与动态聚合（`/api/xblue/*`，Step 05 & Step 06）

- `GET /api/xblue/subgraphs` / `POST /api/xblue/subgraphs` / `PATCH /api/xblue/subgraphs/{slug}` / `DELETE /api/xblue/subgraphs/{slug}`（`DATA_READ` / `STUDIO_EDIT` / `STUDIO_PUBLISH`）：企业主图（`main`）、部门独立子图与隔离开发沙盒子图（`is_dev_sandbox`、`source_template_id`、`last_reset_at`）管理（Step 05 / Step 17）。
- `POST /api/xblue/subgraphs/{slug}/dry-run-materialize`（`STUDIO_EDIT` + `DATA_SYNC`）与 `POST /api/xblue/subgraphs/{slug}/reset-nodes`（`STUDIO_EDIT`）：在 `is_dev_sandbox = true` 的开发沙盒子图命名空间内试灌映射数据、计算派生与聚合指标，或一键清空沙盒运行态节点/关系/时序点且保持企业主图 `main` 零影响（Step 17）。
- `POST /api/xblue/subgraphs/{slug}/merge-preview` / `POST /api/xblue/subgraphs/{slug}/merge`（`STUDIO_EDIT` / `STUDIO_PUBLISH`）：部门子图与主图跨图实体对齐及合并冲突消解（`EXTEND_PROPS` / `KEEP_MAIN` / `OVERWRITE_MAIN`）。
- `GET /api/xblue/schema` / `POST /api/xblue/schema` / `POST /api/xblue/schema/validate`（`DATA_READ` / `STUDIO_PUBLISH` / `STUDIO_EDIT`）：按 `?subgraph=` 查询、直接保存或静态校验模型文档（`SchemaDoc`）。
- `GET /api/xblue/schema/versions` / `GET /api/xblue/schema/versions/{v}` / `GET /api/xblue/schema/versions/{v}/diff/{other}` / `POST /api/xblue/schema/versions/{v}/restore`（`DATA_READ` / `STUDIO_EDIT`）：模型版本历史、结构差异对比与回滚草案生成。
- `GET /api/xblue/drafts` / `POST /api/xblue/drafts` / `POST /api/xblue/drafts/ai` / `GET /api/xblue/drafts/{id}` / `PUT /api/xblue/drafts/{id}` / `POST /api/xblue/drafts/{id}/discard` / `POST /api/xblue/drafts/{id}/publish`（`DATA_READ` / `STUDIO_EDIT` / `STUDIO_PUBLISH`）：模型草案生命周期、正向完备度 `pass_count` 统计、AI 自动建图与咨询锁乐观并发发布（存在未签收 `maturity=PROPOSED` 要素时发布返回 `400 UNCONFIRMED_PROPOSALS_REMAIN`，Step 05 / Step 18）。
- `POST /api/xblue/drafts/{id}/copilot`（`STUDIO_EDIT`）：焦点感知 AI 建模助手，支持 `DIAGNOSE_ONLY`（零事务零副作用完备度体检）与 `PATCH_DRAFT`（单事务先写 `xb_schema_draft_snapshots` 回退点、受限解码打入 `PROPOSED` 补丁并重算 `pass_count` / `issues`）；配套 `GET /api/xblue/drafts/{id}/snapshots`（`DATA_READ`）、`POST /api/xblue/drafts/{id}/snapshots/{snap_id}/restore`（`STUDIO_EDIT`，记录 `STUDIO_SNAPSHOT_RESTORE` 审计）与 `POST /api/xblue/drafts/{id}/confirm-proposals`（`STUDIO_EDIT`，记录 `BATCH_CONFIRM` 快照并确认 `PROPOSED` 要素，Step 18）。
- `GET /api/xblue/subgraphs/{slug}/lineage/{node_type}?draft_id=`（`DATA_READ`）与 `GET /api/xblue/subgraphs/{slug}/spec-doc?draft_id=&format=json|markdown`（`DATA_READ`）：纯确定性计算的单节点左→右五段全链路血缘管道图（`DATASET_COLUMNS → MAPPING_SPEC → NODE_SCHEMA → COMPUTE_RULES → DOWNSTREAM_CONSUMERS`）与 7 章节模型全景规格书（含 `q1..q6` 推荐验收问数集与 Markdown 正文，Step 18）。
- `POST /api/xblue/subgraphs/from-template {pack_id, slug_override?, display_name?}`（`STUDIO_EDIT` → `201 TrialCloneTemplateResponse`）：校验内置模板包 `manifest.json` 与文件 `SHA-256` 完整性，在单租户事务内创建 `trial_<pack>_<shortid>`（或匹配 `^trial_[a-z0-9_-]{1,56}$` 的自定义标识）开发沙盒子图（`is_dev_sandbox = true`），发布初始模型版本并生成 `DRAFT` 工作草案，内存解析样例 CSV 物化沙盒节点、关系与派生/聚合指标，写 `TRIAL_TEMPLATE_CLONE` 审计且保持企业主图 `main` 零影响（Step 19）。
- `POST /api/xblue/subgraphs/{slug}/package-agent {agent_name, description?, node_types?, metric_names?, environment?, expose_mcp?, publish_agent?}`（`STUDIO_EDIT` + `AGENT_EDIT`，`environment="PROD"` 时拦截 `BUILDER` 令牌与开发沙盒子图 → `200 PackageSubgraphAgentResponse`）与 `GET /api/xblue/subgraphs/{slug}/consumption`（`DATA_READ` → `200 SubgraphConsumptionResponse`）：幂等挂载子图 `OBJECT_TYPE` 与 `METRIC` 发布端点（支持同步暴露至 `/api/mcp`）、合成受控领域智能体 `AgentSpec` 并按需发布版本；汇总各 `NodeTypeSpec` 下游已发布端点、领域智能体、孪生驾驶舱与运营告警规则反向消费计数（Step 19）。
- `GET /api/xblue/mappings` / `POST /api/xblue/mappings/preview` / `POST /api/xblue/mappings/{id}/run` / `GET /api/xblue/mappings/{id}/runs`（`DATA_READ` / `STUDIO_EDIT`）：数据集到图谱映射列表、样本试跑预览、主图全量/增量物化触发与运行记录。
- `GET /api/xblue/objects` / `GET /api/xblue/objects/{type_name}` / `GET /api/xblue/objects/{type_name}/{node_key}`（`DATA_READ`）与 `POST /api/xblue/graphql` / `GET /api/xblue/graphql/sdl`（`DATA_READ`，执行 GraphQL `Mutation` 需 `FLOW_RUN`）：主图已发布类型自动暴露的强类型领域对象 REST 与动态 GraphQL Schema（含自动生成的 `<typeCamel>_<actionCamel>` `Mutation`）。
- `POST /api/xblue/objects/{type_name}/{node_key}/actions/{action_name} {inputs?, idempotency_key, expected_version?, dry_run?}`（`FLOW_RUN` → `200 | 202 InvokeObjectActionResponse`）：经 JSON Schema 入参校验、Cedar ABAC 与行级可见性核验、`SELECT ... FOR UPDATE` 乐观版本守卫（冲突返回 `409 VERSION_CONFLICT`）与 `precondition_expr` 前置断言（不满足返回 `422 ACTION_PRECONDITION_FAILED`），在单租户事务内原子更新自身与声明邻居节点/关系（禁止覆写派生指标 `400 DERIVED_METRIC_WRITE_FORBIDDEN`）、写 `xb_metric_series` 与 `ax_action_log` 幂等锁、触发下游四路径重算并记录 `OBJECT_ACTION_INVOKE` 审计（`require_approval=true` 时写 `ax_approvals(OBJECT_ACTION)` 返回 `202`，Step 26）。
- `POST /api/xblue/links/metrics/write {points: [{link_id, metric, ts, value?, value_text?}], source_tag?}`（`DATA_SYNC` → `200 WriteLinkMetricsResponse`）与 `GET /api/xblue/links/{id}/explain/{metric}`（`DATA_READ` → `200 LinkMetricExplainResponse`）：写入关系边时序指标 `xb_link_metric_series` 并触发 `V→V / V→E / E→V / E→E` 四路径拓扑传播，输出含 `{rule_chain, previous_value, current_value, delta, input_snapshot}` 的三轨溯源（Step 26）。
- `POST /api/xblue/subgraphs/{slug}/certify {draft_id?, sample_limit?}`（`STUDIO_EDIT` → `200 RuntimeCertReport`）：静态扫描孤立死指标（`DEAD_KNOWLEDGE`）与高扇出传导风险，在开发沙盒子图运行一轮 `Trial Tick` 实算冒烟并落库 `L0_INVALID ～ L4_CERTIFIED` 认证记录及 `TRIAL_TICK_CERTIFY` 审计（Step 26）。
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

- `GET /api/auth/tokens` / `POST /api/auth/tokens` / `DELETE /api/auth/tokens/{id}`（已认证用户）：列出当前用户个人访问令牌（不含明文）、签发 `axp_<prefix8>_<secret32>`（支持 `token_profile: "GENERAL"|"BUILDER"` 与可选 `web_base_url`；`BUILDER` 令牌禁止授予生产发布与原值读取权限并在生产变更路由拦截为 `403 BUILDER_TOKEN_PROD_FORBIDDEN`，一次性返回明文并审计 `TOKEN_ISSUE`）或吊销令牌（审计 `TOKEN_REVOKE`，Step 09 / Step 17）。
- `GET /api/axagent/providers` / `POST /api/axagent/providers` / `PUT /api/axagent/providers/{id}` / `DELETE /api/axagent/providers/{id}` / `POST /api/axagent/providers/{id}/test`（`DATA_READ | AGENT_RUN | AGENT_EDIT` 可见系统与本人个人提供方；`SYSTEM` 提供方仅 `GUARD_ADMIN | OPS_ADMIN` 可写，非管理员读取返回 `is_readonly=true`；`allow_private_network=true` 或 `allow_pii=true` 需 `GUARD_ADMIN`）：多模型提供方注册、密钥环加密存储、`structured_mode`（`NONE | JSON_OBJECT | JSON_SCHEMA`）受限解码分级、`deterministic_seed`、`is_local_model`、`max_context_tokens` 配置与受限解码零温连通性核验（Step 09 / Step 15）。
- `GET /api/axagent/llm-preferences` / `PUT /api/axagent/llm-preferences`（已认证用户）：读取或按用途（`CHAT | EMBED | NL2SQL | EXTRACT | AGENT | SUMMARIZE`）保存当前用户个人默认模型提供方绑定，优先于系统级优先级选路并在失败时自动回落，写 `POLICY_CHANGE` 审计（Step 15）。
- `GET /api/axagent/usage?from=&to=`（`AGENT_EDIT`）：按提供方、日期与用途查询请求量、错误数与 Token 用量统计（`ax_llm_usage`）。
- `GET /api/axagent/endpoints` / `POST /api/axagent/endpoints` / `PUT /api/axagent/endpoints/{id}` / `DELETE /api/axagent/endpoints/{id}` / `POST /api/axagent/endpoints/{id}/promote`（`DATA_READ` / `STUDIO_PUBLISH`；`kind=FLOW` 需 `FLOW_PUBLISH`）：管理五类发布端点（`SAVED_QUERY`、`METRIC`、`OBJECT_TYPE`、`FLOW`、`AGENT`），自动推导 Draft 2020-12 `input_schema`，并在提升至 `PROD` 时固定 `target_version` 与 `target_snapshot`。
- `GET /api/axagent/endpoints/openapi.json?environment=PROD` / `GET /api/axagent/endpoints/{id}/skill.json`（`DATA_READ`）：导出 OpenAPI 3.1 规范文档与外部智能体 Skill 清单。
- `POST /api/axagent/apps/{endpoint_id}/run`（端点 `required_scope`）：查询即应用内部执行入口，以当前登录主体直接调用目标能力。
- `POST /api/published/{env}/{tenant_id}/{slug}` / `GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}`（`Authorization: Bearer axp_...` 个人访问令牌）：校验租户与 `required_scope`、`(endpoint_id, token_id)` 速率限制、JSON Schema 入参、`moka` TTL 缓存后执行能力并写审计日志。
- `POST /api/mcp` / `GET /api/mcp` / `DELETE /api/mcp`（`Authorization: Bearer axp_...` 个人访问令牌）：Streamable HTTP MCP Server（`Mcp-Session-Id` 会话管理），同时提供运行态消费平面（`PROD` 且 `expose_mcp=true` 的发布端点、7 个内置只读工具与 `axpoint://datasets/*`、`axpoint://objects/*` 资源）与 `AxBuilder` 构建态平面（按细粒度权限动态暴露 `axbuilder_preflight`、`axbuilder_pull_workspace`、`axbuilder_validate_schema`、`axbuilder_push_schema_draft`、`axbuilder_clone_template`、`axbuilder_upsert_flow_draft`、`axbuilder_run_dev_subgraph`、`axbuilder_reset_dev_subgraph`、`axbuilder_upsert_dashboard_draft` 共 9 个构建工具及 `axpoint://templates/{id}`、`axpoint://subgraphs/{slug}` 资源，统一回传 `deep_links` 并对草稿/沙盒写操作记录 `BUILDER_MCP_MUTATE` 审计，Step 09 / Step 17）。
- `GET /api/axagent/mcp-clients` / `POST /api/axagent/mcp-clients` / `DELETE /api/axagent/mcp-clients/{id}` / `POST /api/axagent/mcp-clients/{id}/sync`（`AGENT_EDIT`）：外部 Streamable HTTP MCP 服务登记（经 `NetGuard` 校验）与 `tools/list` 同步缓存。
- `GET /api/axagent/agents` / `POST /api/axagent/agents` / `GET /api/axagent/agents/{id}` / `PUT /api/axagent/agents/{id}` / `DELETE /api/axagent/agents/{id}` / `POST /api/axagent/agents/{id}/publish`（`DATA_READ` / `AGENT_EDIT`）：智能体草稿编辑、多智能体嵌套深度校验（`<= 2`）与不可变版本发布（`ax_agent_versions`）。
- `POST /api/axagent/agents/{id}/chat`（`AGENT_RUN`；`draft=true` 需 `AGENT_EDIT`）：智能体多轮工具循环对话（支持 `Accept: text/event-stream` 流式 `token|tool_call|tool_result|approval_required|final|error` 事件与 `Accept: application/json` 同步返回，写 `ax_messages` 与 `AGENT_CHAT` 审计）。
- `GET /api/axagent/conversations` / `GET /api/axagent/conversations/{id}/messages` / `DELETE /api/axagent/conversations/{id}`（`AGENT_RUN`，仅本人会话）：工作台多轮会话与消息流管理。
- `POST /api/axagent/agents/{id}/embeds` / `GET /api/axagent/agents/{id}/embeds` / `DELETE /api/axagent/embeds/{embed_id}`（`VIEW_SHARE`）与 `POST /api/axagent/embed/{token}/chat`（公开嵌入令牌 `axe_...`）：按权限子集与 `allowed_origins` 签发、查询、吊销外部嵌入链接及按 `(embed_id, client_ip)` 限流的公开对话入口。
- `GET /api/axagent/im-connectors` / `POST /api/axagent/im-connectors` / `PATCH /api/axagent/im-connectors/{id}` / `DELETE /api/axagent/im-connectors/{id}` / `POST /api/axagent/im-connectors/{id}/test`（`DATA_READ | AGENT_EDIT` 读取，变更与探测需 `AGENT_EDIT`，`config.allow_private_network=true` 需 `GUARD_ADMIN`）：管理飞书（`FEISHU`）、钉钉（`DINGTALK`）与企业微信（`WECOM`）零入站端口出站长连接双向机器人、30s 单主租约心跳状态与连通性探测（密钥密封于 `ds_credentials`，Step 21）。
- `GET /api/axagent/im-bindings` / `DELETE /api/axagent/im-bindings/{id}`（`DATA_READ` 仅见本人，`AGENT_EDIT` 可见并管理全租户）与 `GET /api/axagent/im-bind/preview?token=` / `POST /api/axagent/im-bind/confirm`（已认证用户）：预览与原子核销 10 分钟一次性 IM 私聊挑战码 `axim_...`，将外部 IM 身份绑定至平台 `sec_users` 主体（记录 `IM_ACCOUNT_BIND` 审计，强制剥离 `PII_RAW_READ` 并全量继承行级策略与列脱敏，Step 21）。

### `AxSim` 写时复制沙盘、因果推演、对比问数与决策下发（`/api/axsim/*`，Step 10）

- `POST /api/axsim/sandboxes/preflight {subgraph_slug?, scope: SandboxScope}`（`SIM_RUN` → `200 ScopePreflightReport`）：只读采样候选范围节点与跨界边，静态评估 `readiness_grade`（`READY | DEGRADED | BLOCKED`）、`causal_edge_coverage_ratio`、`agg_rule_closure_ratio`、`isolated_node_ratio`、`missing_upstream_types`、`truncated_edge_types` 与 `reason_codes`（Step 20）。
- `GET /api/axsim/sandboxes` / `POST /api/axsim/sandboxes` / `GET /api/axsim/sandboxes/{id}` / `DELETE /api/axsim/sandboxes/{id}`（`DATA_READ` / `SIM_RUN`）：按范围（`scope: {root_node_ids, depth<=6, direction, type_names, link_types, max_nodes<=50000}`）从生产业务图克隆基线快照到 `ax_sim_base_*`，管理沙盘生命周期（`CLONING | READY | ARCHIVED`）。
- `GET /api/axsim/sandboxes/{id}/graph`（`DATA_READ`）：返回 `base` 与 `overlay` 合并后的沙盘节点与关系视图（含 `is_modified`、`is_new`、`overlay_hash`）。
- 覆盖层编辑（`SIM_RUN`）：`PATCH /api/axsim/sandboxes/{id}/nodes/{node_id}`、`POST /api/axsim/sandboxes/{id}/nodes`、`DELETE /api/axsim/sandboxes/{id}/nodes/{node_id}`、`POST /api/axsim/sandboxes/{id}/links`、`PATCH /api/axsim/sandboxes/{id}/links/{link_id}`、`DELETE /api/axsim/sandboxes/{id}/links/{link_id}`、`POST /api/axsim/sandboxes/{id}/reset`、`POST /api/axsim/sandboxes/{id}/refresh-base`（保留覆盖层并列出孤立 ID）；所有操作不修改生产表。
- `POST /api/axsim/sandboxes/{id}/simulate`（`SIM_RUN`）：快速因果推演，支持 `{actions, scheduled_events?, goal_constraints?, params?}`，支持 `RATIO | ADD | ABSOLUTE` 及 `delta_ratio` 简写，返回 `fired_events` 与 `deadline_report`；目标节点缺失指标且模式为 `RATIO`/`ADD` 返回 `400 INVALID_ARGUMENT`（Step 10 / Step 28）。
- `GET /api/axsim/sandboxes/{id}/scenarios` / `POST /api/axsim/sandboxes/{id}/scenarios` / `GET /api/axsim/scenarios/{id}` / `POST /api/axsim/scenarios/{id}/rerun` / `DELETE /api/axsim/scenarios/{id}`（`DATA_READ` / `SIM_RUN`）：创建并求解稳态（SCC + 阻尼雅可比迭代）或分步（`t = 0..=horizon_steps`）场景，支持 `AT_STEP` 定时事件与 `METRIC_THRESHOLD` 条件触发事件队列、`SimGoalConstraint` 红线监测及反向可达最小延迟 `deadline_step = breach_step - min_lag - 1` 求解，可选 Monte-Carlo 不确定性抽样（`P10`/`P90`），记录 `overlay_hash` 并在覆盖层变化后标为过期（Step 10 / Step 28）。
- `GET /api/axsim/scenarios/{id}/impacts?type_name=&metric=&limit=&cursor=` / `GET /api/axsim/scenarios/{id}/checkpoints`（`DATA_READ`）：分页查询受影响节点（含 `before_val`、`after_val`、`delta`、`delta_ratio`、`p10`、`p90`、`hop_distance`、`top_path`、`series`）及逐步回放检查点。
- `GET /api/axsim/scenarios/compare?ids=&metrics=&type_name=&briefing=`（`DATA_READ`）：同沙盘内 2～5 个场景 KPI 与节点级并排对比；`briefing=true` 返回经数值校验的 AI 研判 `{summary, recommended_scenario_id, reasons[], risks[]}`。
- `POST /api/axsim/ask`（`SIM_RUN`）：推演问数，自动识别 `WHAT_IF`（大模型解析干预 + 节点检索消歧 + 创建运行场景）或 `QUERY_RESULT`（内存 DuckDB `impacts` 表 NL2SQL），写 `ASK_DATA` 审计。
- `POST /api/axsim/scenarios/{id}/decisions` / `GET /api/axsim/decisions/{id}` / `POST /api/axsim/decisions/{id}/rebase`（`SIM_RUN`，审批需 `SIM_APPLY`）：创建下发提案与 `ax_approvals(source_kind=SIM_APPLY)` 审批门；通过后 `SELECT ... FOR UPDATE` 校验生产基线冲突，无冲突则原子写 `xb_nodes.metrics`、`xb_metric_series(ACTION)`、`ax_action_log`、`ax_outbox(sim.applied, node.metrics_changed)`、`sec_audit_logs(SIM_APPLY)` 并按需触发 `PROD` 流程；存在冲突则标为 `CONFLICT` 并支持 `/rebase`。

### `xBlueView` 孪生驾驶舱、AI 看板生成、空间绑定与动作回写（`/api/xblueview/*`，Step 11）

- `GET /api/xblueview/dashboards?limit=&cursor=`（`DATA_READ` → `200 PaginatedList<DashboardRecord>`）/ `POST /api/xblueview/dashboards`（`VIEW_EDIT` → `201 DashboardDetail`，支持 `zone_layout?: "FREE_GRID"|"STANDARD_FIVE_ZONE"` 初始化 12 列五区语义骨架）/ `POST /api/xblueview/dashboards/{id}/apply-zone-preset {zone_layout}`（`VIEW_EDIT` → `200 DashboardDetail`）/ `GET /api/xblueview/dashboards/{id}` / `GET /api/xblueview/dashboards/by-slug/{slug}`（`DATA_READ` → `200 DashboardDetail`）/ `PUT /api/xblueview/dashboards/{id}`（`VIEW_EDIT` → `200 DashboardRecord`）/ `DELETE /api/xblueview/dashboards/{id}`（`VIEW_EDIT` → `204`）：驾驶舱增删改查；无 `VIEW_EDIT` 主体仅可见 `PUBLISHED` 驾驶舱（访问 `DRAFT` 返回 `404`），`slug` 校验 `^[a-z0-9][a-z0-9-]{0,62}$`（重名返回 `409 NAME_TAKEN`），`filters` 为 `FilterDef[] {key, label, kind, field_map: {<widget_id>: "<列名>"}, options_source?, default?}`（Step 11 / Step 20）。
- `POST /api/xblueview/dashboards/ai-generate {prompt, title?, slug?, theme?}`（`VIEW_EDIT` → `201 DashboardDetail`）与 `POST /api/xblueview/dashboards/ai-draft {name, slug?, prompt, subgraph_slug?, seed_node_types?, theme?}`（`VIEW_EDIT` → `201 DashboardDetail`）：收集目标子图（默认主图 `main`）已发布 `SchemaDoc`（可选限定 `seed_node_types` 种子实体）、语义指标 `an_metrics`、就绪数据集 `ds_datasets`（列来自目录快照）、`PROD` 流程与空间资产摘要，经 `LlmRouter(Purpose::Extract)` 生成栅格草案；服务端校验栅格不越界、引用对象存在且 `SQL` 通过 `sql_guard`（失败自动附带问题列表让模型自修 1 次），在租户事务中落库 `DRAFT` 驾驶舱与全部组件（预计算 `deps`，Step 11 / Step 19）。
- `POST /api/xblueview/dashboards/{id}/preflight`（`VIEW_EDIT | VIEW_SHARE` → `200 DashboardPreflightReport {dashboard_id, status: "PASS"|"WARN"|"BLOCKED", checked_widgets, blocking_count, warning_count, issues}`）：扫描看板全部组件与全局筛选数据源的栅格边界、数据集/指标/保存查询/空间资产/推演场景/`PROD` 动作流程及发布者权限；`POST /api/xblueview/dashboards/{id}/publish` 与针对驾驶舱的 `POST /api/xblueview/shares` 在存在 `BLOCKING` 项时拦截并返回 `422 VIEW_PREFLIGHT_BLOCKED`（`details.issues` 返回诊断列表，Step 20）。
- `POST /api/xblueview/dashboards/{id}/publish`（`VIEW_EDIT` → `200 DashboardRecord`，`version + 1`、`status = "PUBLISHED"`）/ `POST /api/xblueview/dashboards/{id}/duplicate {name?, slug?}`（`VIEW_EDIT` → `201 DashboardDetail`，重映射 `filters[].field_map` 中的组件 ID）/ `PUT /api/xblueview/dashboards/{id}/layout {items: [{widget_id, position: {x,y,w,h}}]}`（`VIEW_EDIT` → `200 WidgetRecord[]`，校验 `x + w <= canvas.cols`）。
- `POST /api/xblueview/dashboards/{id}/widgets`（`VIEW_EDIT` → `201 WidgetRecord`）/ `PUT /api/xblueview/widgets/{id}`（`VIEW_EDIT` → `200 WidgetRecord`）/ `DELETE /api/xblueview/widgets/{id}`（`VIEW_EDIT` → `204`）：支持 20 种 `WidgetKind` 与 13 类 `WidgetDataSource`（`kind` 标签：`SAVED_QUERY`、`METRIC`、`SQL`［湖表使用一段式数据集名如 `demo_orders`，平台视图使用 `ax.*`］、`NODE_METRIC`、`PIVOT`、`GRAPH`、`ALERTS`、`SIM_SCENARIOS`、`NODES_GEO`、`FLOOR_PLAN`、`SCENE_3D`、`ACTION`、`STATIC`），保存时自动解析并写入 GIN 索引列 `deps {dataset_ids, type_names, metrics, alerts, scenario_ids}`。
- `GET /api/xblueview/charts?limit=&cursor=`（`DATA_READ` → `200 PaginatedList<SavedChartRecord>`）/ `POST /api/xblueview/charts`（`VIEW_EDIT` → `201 SavedChartRecord`，写 `CHART_PUBLISH` 审计）/ `GET /api/xblueview/charts/{id}`（`DATA_READ` → `200 SavedChartRecord`）/ `PUT /api/xblueview/charts/{id}`（`VIEW_EDIT` → `200 SavedChartRecord`）/ `DELETE /api/xblueview/charts/{id}`（`VIEW_EDIT` → `200 {deleted: true}`）/ `POST /api/xblueview/charts/{id}/pin {dashboard_id, target_zone?, width?, height?}`（`VIEW_EDIT` → `201 PinChartToDashboardResponse {widget_id, semantic_zone, widget, dashboard_id, dashboard_slug, deep_link}`）：持久化可复用可视化图表资产并原子寻址钉入目标驾驶舱（Step 25）。
- `POST /api/xblueview/widgets/{id}/data {filters: {<filter_key>: <val>}, drill_path: [{field, value}]}` / `POST /api/xblueview/widgets/preview {widget_kind, data_source, encoding?, options?, drill?, filters?, drill_path?}`（`DATA_READ` → `200 WidgetDataResponse {columns: [{name, data_type}], rows, row_count, truncated, generated_at, data_version, drill_level}`）：
  - `filters` 以仪表盘 `FilterDef.key` 为键，按 `field_map[widget_id]` 注入参数化过滤，且保持子查询包装后的列级 PII 脱敏血缘不降级；
  - 固定列契约：`SCENE_3D` 输出 11 列 `[node_id, type_name, node_key, title, mesh_name, pos_x, pos_y, pos_z, scale, metric_value, open_alerts]`；`FLOOR_PLAN` 输出 8 列 `[node_id, type_name, node_key, title, x_ratio, y_ratio, metric_value, open_alerts]`；`NODES_GEO` 输出节点 8 列 `[node_id, type_name, node_key, title, lng, lat, metric_value, open_alerts]`（配置 `link_types` 时追加飞线关系列 `record_kind, link_type, src_lng, src_lat, dst_lng, dst_lat, weight`）；`GRAPH` 输出 10 列 `[record_kind("NODE"|"LINK"), id, type_name, node_key, title, metric_value, src_node_id, dst_node_id, link_type, weight]`；`ALERTS` 输出 10 列 `[id, rule_id, severity, status, title, node_id, dataset_id, first_seen_at, last_seen_at, occurrences]`；`ACTION` 输出最近 10 条执行记录 7 列 `[id, action_kind, target, status, payload, error_message, created_at]`（综合 `WRITE_NODE` 与 `RUN_FLOW`）；`PIVOT` 附带 `_row_kind`（`DETAIL | SUBTOTAL | TOTAL`）；
  - 缓存键 `(tenant_id, dashboard_id, widget_id, hash(filters, drill_path, principal_fingerprint))`（`moka` 容量 10,000，TTL 600s）。
- `POST /api/xblueview/widgets/{id}/invoke {inputs, target_node_id?, idempotency_key?}`（`FLOW_RUN` → `200 InvokeWidgetActionResponse {mode, run_id, status, node_id, action_log_id, idempotency_replay}`；`ShareContext` 一律返回 `403 PERMISSION_DENIED`；无 `VIEW_EDIT` 主体不可调用 `DRAFT` 驾驶舱组件）：按 `input_schema` 校验入参（失败返回 `400 SCHEMA_INVALID`）；`RUN_FLOW` 启动目标流程 `PROD` 部署并写 `ax_action_log` 与 `FLOW_RUN` 审计；`WRITE_NODE` 校验部门 ABAC 与主图 `NodeTypeSpec` 后在单事务内更新 `xb_nodes`、`xb_metric_series(source_tag='ACTION')`、`ax_action_log(source_kind='VIEW')` 并外发 `node.upserted` / `node.metrics_changed`。
- `GET /api/xblueview/dashboards/{id}/stream`（`DATA_READ`，SSE `text/event-stream`）：经 `ViewEventSubscriber` + `pg_notify('ax_view_events')`（>7,000B 自动压缩为 `dashboard_ids` 并在全实例同步失效缓存）+ `PgListener` 推送 `event: invalidate`（`{"widget_ids":[...]}`）与 `event: alert`，15s `KeepAlive`。
- `GET /api/xblueview/assets?asset_kind=&limit=&cursor=`（`DATA_READ`）/ `POST /api/xblueview/assets`（`VIEW_EDIT` → `201 AssetRecord`，`multipart/form-data` 字段 `asset_kind`、`name?`、`file`；超限返回 `413 UPLOAD_TOO_LARGE`；SVG 经严格白名单清洗并以 `CSP: sandbox` 隔离返回，`.glb` 校验 12 字节 `glTF` v2 头与长度，`GEOJSON` 校验 `FeatureCollection`）/ `GET /api/xblueview/assets/{id}/content`（`DATA_READ`）/ `DELETE /api/xblueview/assets/{id}`（`VIEW_EDIT` → `204`）。
- `GET /api/xblueview/assets/{id}/placements` / `PUT /api/xblueview/assets/{id}/placements {placements: [{node_id, x_ratio, y_ratio}]}` 与 `GET /api/xblueview/assets/{id}/bindings3d` / `PUT /api/xblueview/assets/{id}/bindings3d {bindings: [{node_id, mesh_name, pos_x, pos_y, pos_z, scale}]}`（`DATA_READ` / `VIEW_EDIT`）：读写 2D 平面图比例坐标与 3D `glTF` 网格/桩点绑定，保存时自动重算关联组件 `deps` 并失效 `ViewCache`。

### `xBlueView` 全格式出品引擎与安全分享（`/api/xblueview/*`，Step 12）

- `POST /api/xblueview/exports {export_format, source, options?, template_id?}`（`EXPORT_DATA` → `201 ExportJobRecord`）/ `GET /api/xblueview/exports?mine=&limit=&cursor=` / `GET /api/xblueview/exports/{id}`（`EXPORT_DATA` → `200`）/ `DELETE /api/xblueview/exports/{id}`（`EXPORT_DATA` → `204`）：提交后台 `ax_jobs(kind=EXPORT)` 异步出品任务，支持 `XLSX | CSV | PDF | DOCX | PPTX | MARKDOWN` 与六种来源（`DASHBOARD | WIDGET | SAVED_QUERY | PIVOT | SCENARIO | CONVERSATION`；`SAVED_QUERY` 对 `CSV`/`XLSX` 支持最高 `1,000,000` 行，超过 `50,000` 行经临时 `Parquet` 流式传输），以创建者身份取数并强制执行 PII 脱敏与水印（`options.watermark = true` 生成 `"{username} YYYY-MM-DD HH:MM"`），完成后计算 `expires_at`、外发 `export.finished` 并写 `EXPORT_FILE` 审计；`RETENTION(EXPORT_FILE)` 自动删除过期产物并置为 `EXPIRED`。
- `GET /api/xblueview/exports/{id}/download`（`EXPORT_DATA`）：仅允许创建者或具备 `DATA_ADMIN` 主体下载，采用 `ReaderStream` 流式传输并携带 RFC 5987 `Content-Disposition: attachment; filename*=UTF-8''...`。
- `GET /api/xblueview/export-templates`（`EXPORT_DATA`）/ `POST /api/xblueview/export-templates`（`VIEW_EDIT` → `201 ExportTemplateRecord`，`multipart/form-data` 字段 `export_format("DOCX"|"PPTX")`、`name?`、`file`，自动解析 `{{key}}` 占位符）/ `DELETE /api/xblueview/export-templates/{id}`（`VIEW_EDIT` → `204`）。
- `POST /api/xblueview/shares {target_kind, target_id, expires_in_hours(1..=720), max_views?, password?, locked_filters?, allow_filters?, allow_export?}`（`VIEW_SHARE` → `201 CreateShareResponse {id, token, url, expires_at}`）/ `GET /api/xblueview/shares?target_id=&limit=&cursor=` / `DELETE /api/xblueview/shares/{id}`（`VIEW_SHARE`）：签发一次性返回明文的 `axs_<prefix8>_<secret32>` 令牌（数据库仅存 `SHA-256` 哈希与 `argon2id` 口令哈希，`DASHBOARD` 要求 `PUBLISHED`），写 `SHARE_CREATE` 审计；`axs_` 令牌访问任何普通业务接口一律返回 `401 TOKEN_INVALID`。
- 公开分享接口（`ShareContext`，无 `AuthPrincipal`，按 IP 限流 `120 rpm`、解锁 `10 rpm`，响应头强制 `Referrer-Policy: no-referrer` 与 `X-Robots-Tag: noindex`）：
  - `GET /api/xblueview/share/{token}`：经 `ax_resolve_share` 解析，不存在/过期/撤销/超 `max_views`/创建者失去 `DATA_READ` 一律返回 `404 SHARE_NOT_FOUND`；受口令保护且未携带有效 `X-Share-Session` 时返回 `{password_required: true}`（不扣减次数），否则原子递增 `view_count`、写 `SHARE_ACCESS`（`username = "share:<prefix>"`）并返回目标只读元数据；
  - `POST /api/xblueview/share/{token}/unlock {password}`：校验 `argon2id` 口令并返回 2 小时有效期的 HMAC 签名 `share_session`（超限返回 `429 RATE_LIMITED`）；
  - `POST /api/xblueview/share/{token}/widgets/{widget_id}/data {filters, drill_path}`：校验组件属于被分享驾驶舱，`locked_filters` 强制覆盖同名筛选，`allow_filters=false` 时忽略其余筛选，以分享创建者身份取数；
  - `GET /api/xblueview/share/{token}/download?format=`：`EXPORT_FILE` 目标直接流式下载；`DASHBOARD` / `SCENARIO` 目标在 `allow_export=true` 时支持 `?format=PDF|XLSX|CSV|DOCX|PPTX|MARKDOWN`（缺省 `PDF`）即时渲染带水印产物下载。

### `AxGuard` 认证会话与完整安全治理（`/api/auth/*`、`/api/axguard/*`，Step 13）

- 认证与会话生命周期（`/api/auth/*`）：
  - `GET /api/auth/config`（公开）→ `AuthConfigResponse {env, oidc_enabled, oidc_display_name?, default_tenant}`；`GET /api/auth/jwks.json`（公开）输出全部活跃 `Ed25519` 验证公钥（JWK Set）。
  - `POST /api/auth/login {tenant?, username, password}`（公开，按 IP 限流 `10 rpm`）：经 `ax_lookup_login` 与 `Argon2id(m=19456, t=2, p=1)` 恒定耗时校验（连续 5 次失败锁定 15 分钟），签发 900s `EdDSA` 访问令牌并通过 `Set-Cookie: axp_refresh=...; HttpOnly; SameSite=Strict; Path=/api/auth` 下发 14 日刷新令牌，写 `LOGIN` 审计。
  - `POST /api/auth/refresh`（按 IP 限流 `60 rpm`）：经 `ax_resolve_session` 执行单事务会话轮换；若检测到已轮换或已吊销令牌重放，立即熔断整条 `family_id` 并写 `TOKEN_REVOKE(reason="refresh_reuse")` 审计返回 `401 TOKEN_INVALID`。
  - `POST /api/auth/logout` / `POST /api/auth/logout-all` / `POST /api/auth/password {old_password, new_password}`：分别吊销当前会话族、吊销用户全部会话并推进 `tokens_valid_after`、修改口令（≥12 位且不含用户名）并强制失效历史令牌。
  - `GET /api/auth/oidc/start?tenant=&return_to=` / `GET /api/auth/oidc/callback?code=&state=`（按 IP 限流 `30 rpm`）：执行一次性 `state` + PKCE `S256` + ID Token 校验、自动开户与角色/部门映射同步。
- 安全治理控制台（`/api/axguard/*`）：
  - 用户、角色、部门与会话（`GUARD_ADMIN`）：`GET|POST /api/axguard/users`、`PATCH /api/axguard/users/{id}`、`POST /api/axguard/users/{id}/reset-password`、`PUT /api/axguard/users/{id}/attributes`；`GET|POST /api/axguard/roles`、`PATCH|DELETE /api/axguard/roles/{id}`；`GET /api/axguard/departments`、`PUT|DELETE /api/axguard/departments/{dept_id}`；`GET /api/axguard/sessions`、`DELETE /api/axguard/sessions/{family_id}`。
  - Cedar ABAC 与行列级策略（`GUARD_ADMIN`）：`GET /api/axguard/policies`、`PUT|DELETE /api/axguard/policies/{name}`、`POST /api/axguard/policies/validate`、`POST /api/axguard/policies/simulate`；`GET|POST /api/axguard/row-policies`、`PUT|DELETE /api/axguard/row-policies/{id}`、`GET|PUT /api/axguard/row-policy-defaults`、`POST /api/axguard/row-policies/preview`、`POST /api/axguard/row-policies/simulate-state`（支持 `policy_mode: "STATIC_ATTR"|"STATE_DRIVEN"`、`metrics.*` / `properties.*` 实时状态谓词与 `ttl_timestamp_field` / `ttl_max_age_hours` 活跃期过滤，并在 `AxSim` 沙盘克隆与读取中同源继承，Step 13 / Step 27）；`GET|PUT /api/axguard/column-policies`、`DELETE /api/axguard/column-policies/{id}`。
  - 跨部门授权与防篡改审计链：`GET|POST /api/axguard/grants`、`DELETE /api/axguard/grants/{id}`（`DATA_READ` / `GUARD_ADMIN`，关联 `ax_approvals(source_kind='GRANT')`）；`GET /api/axguard/audit`、`GET /api/axguard/audit/export.csv`、`GET /api/axguard/audit/verify`、`GET /api/axguard/audit/stats`（`GUARD_ADMIN`）。

### `AxOps` 运维监控、备份恢复、行业模板包与工作项（`/api/axops/*`，Step 14）

- 任务队列、构建引导、深度健康与保留策略：
  - `GET /api/axops/onboarding-status`（`DATA_READ` → `200 OnboardingStatusResponse`）：单次只读租户事务实时统计 `DATA_INGEST`、`NODE_MODELING`、`MAPPING_AND_FLOW`、`SIM_VERIFICATION`、`COCKPIT_AND_AGENT` 五阶段资产计数、完成百分比 `progress_pct`、首个未完成阶段推荐路由与演练沙盒子图标记（Step 19）。
  - `GET /api/axops/jobs`、`GET /api/axops/jobs/{id}`（`DATA_READ`）；`POST /api/axops/jobs/{id}/retry`、`POST /api/axops/jobs/{id}/cancel`、`POST /api/axops/jobs/rollup-now`（`OPS_ADMIN`）。
  - `GET /api/axops/health/deep`、`GET /api/axops/outbox/stats`、`GET /api/axops/system/info`（`OPS_ADMIN`）：每项探针独立 `3s` 超时，返回 `DeepHealthReport {database, parser, lake, llm, embed, disk, jobs, outbox, migrations, version}`。
  - `GET /api/axops/retention`、`PUT /api/axops/retention/{target} {keep_days}`、`POST /api/axops/retention/run-now`（`OPS_ADMIN`）：覆盖全部 11 类保留目标并立即入队 `RETENTION` 任务。
- 平台级备份与行业模板包：
  - `GET|POST /api/axops/backups`、`GET /api/axops/backups/{id}`（仅 `default` 租户 `OPS_ADMIN`）：创建 `PENDING` 备份并写 `BACKUP_RUN` 审计，由 `axpoint-ops`（或本地 `AXPOINT_BACKUP_CMD`）执行 `backup.sh` 并支持 `restore-drill.sh` 演练核验为 `VERIFIED`。
  - `GET /api/axops/templates`、`GET /api/axops/templates/{pack_id}`、`GET /api/axops/templates/{pack_id}/preview`（`DATA_READ`，预览返回 `TemplatePreviewResponse {manifest, readme_markdown, topology_summary, sample_metrics, dashboard_previews, sample_rows, agents}`）；`POST /api/axops/templates/{pack_id}/plan {on_conflict?}`、`POST /api/axops/templates/{pack_id}/install {dry_run?, on_conflict?}`、`GET /api/axops/templates/installs`、`POST /api/axops/templates/installs/{id}/apply`、`POST /api/axops/templates/installs/{id}/uninstall`（安装清单读取 `DATA_READ`，规划/全量安装/卸载 `OPS_ADMIN`）：校验 `sha256`、支持冲突 `SKIP`/`RENAME`、`dry_run` 仅规划、断点续装与内容哈希一致时的安全卸载（Step 14 / Step 19）。
  - `POST /api/axops/demo/{scenario}/load`（`OPS_ADMIN`，`supply_chain | manufacturing_equipment | retail_store`）：一键安装模板包、运行刷新流程、回放 30 日时序并创建预置推演沙盘与场景。
- 交付追踪工作项：
  - `GET /api/axops/work-items`、`GET /api/axops/work-items/{id}`（`DATA_READ`）；`POST /api/axops/work-items`、`PATCH /api/axops/work-items/{id}`、`POST /api/axops/work-items/{id}/comments`（`FLOW_RUN`）：租户内 `WI-<n>` 自增编号，支持手动、`AxFlow` `ACTION_EMIT(WORK_ITEM)`、告警转工作项（告警解决自动追加 `LINK` 事件）、推演决策与模板待办五类来源。

---

## 2. AxParser Sidecar（`axpoint-parser`，端口 `8001`，仅内部网络）

除 `GET /health` 外，所有请求携带 `X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN` 与 `X-AxPoint-Tenant`（可选 `X-AxPoint-Timezone`、`traceparent`），否则返回 `401`。

### `GET /health`
返回解析引擎版本（`version`）与模型目录（`${AXPOINT_MODEL_PATH:-/var/lib/axpoint/models}`）就绪状态。

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

### 服务端图表渲染与全格式出品内部接口（Step 12）

- `POST /render/chart {option, width, height, format: "svg"|"png", theme?}`：由常驻 Node.js 24 子进程池（`echarts` SSR + `@resvg/resvg-js` 2x 像素密度 + `NotoSansCJK` 字体，单次超时 15s 自动重启子进程并返回 `504 UPSTREAM_TIMEOUT`）直接输出 `image/svg+xml` 或 `image/png` 二进制流。
- `POST /export/{format} {tenant_id, job_id, document}`（`format` 为 `xlsx | csv | pdf | docx | pptx | markdown`，大小写不敏感）：按 `ExportDocument`（兼容 `rows_parquet_path` 流式读取及用户 `DOCX`/`PPTX` 模板替换）先写 `.part` 再原子重命名至 `${AXPOINT_DATA_ROOT}/exports/<tenant>/<job_id>.<ext>`，返回 `SidecarExportResponse {file_path, file_size, sha256, file_name}`。

### PII 智能识别与批量脱敏内部接口（Step 13）

- `POST /pii/analyze {texts, language: "zh", score_threshold: 0.6}`：基于 Presidio + spaCy 中文模型与自定义校验识别器（GB 11643 身份证、Luhn 银行卡、中国手机号、邮箱、省市区门牌地址、人名、密钥/PEM 头），返回每段文本的 `PiiSpanModel[] {category, start, end, score}`。
- `POST /pii/anonymize {texts, rules, language?, score_threshold?, tenant_id?}`：按 §13.7 统一脱敏规范对命中区间执行脱敏替换并返回 `PiiAnonymizeResponse {texts}`。
