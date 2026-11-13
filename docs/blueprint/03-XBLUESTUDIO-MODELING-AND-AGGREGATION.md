# 03 · xBlueStudio 可视化节点建模与动态聚合引擎（Step 05 ~ Step 06）

本文件定义 **xBlueStudio（可视化节点与关系设计器、AI 自动模型发现、数据源映射绑定与人工确认发布）** 及 **动态指标聚合引擎（Rollup & Knowledge-to-Node Sync）** 的完整实现规范。

---

## Step 05：xBlueStudio 可视化画布、数据源映射与 AI 辅助建模

### 5.1 核心业务闭环
1. **可视化设计（面向业务）**：在交互式画布上拖拽创建业务节点类型（`BlueNodeTypeDef`）、连线定义因果/业务关系（`BlueEdgeTypeDef`）、配置属性、主键与聚合规则。
2. **AI 自动发现模型（Schema / Document → xBlueNode Schema）**：用户选中若干数据集（`ds_datasets`）或文档超图（`hg_docs`），由 AI 结合列元数据画像与高频三元组自动生成**业务节点模型草案（Draft Schema + Mapping Rules）**。
3. **人机确认发布（Human Confirmation）**：所有 AI 生成的草案均以 `status = 'DRAFT'` 保存，业务/技术人员在画布微调并确认口径一致后点击发布，转为 `PUBLISHED` 并激活映射与聚合规则。

### 5.2 数据库 Schema（`deploy/migrations/003b_xbluestudio_mappings.sql`）

```sql
-- 数据集 -> 业务节点/关系映射规则表
CREATE TABLE IF NOT EXISTS xb_data_mappings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    dataset_id      UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    target_kind     TEXT NOT NULL, -- 'NODE' | 'LINK'
    target_type     TEXT NOT NULL, -- 对应 xb_node_types.name 或 xb_edge_types.name
    key_column      TEXT NOT NULL, -- 生成 node_key 的主键列（LINK 时为 src_key_column）
    dst_key_column  TEXT,          -- LINK 时必填，目标节点主键列
    src_type_name   TEXT,          -- LINK 时必填
    dst_type_name   TEXT,          -- LINK 时必填
    title_column    TEXT,          -- 生成节点标题的列
    field_mappings  JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{source_col, target_prop, transform_expr?, is_metric}]
    filter_sql      TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'PUBLISHED',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE INDEX IF NOT EXISTS idx_xb_mappings_dataset ON xb_data_mappings (tenant_id, dataset_id);
```

### 5.3 AI 自动建模发现流程（`axpoint-llm` + `axpoint-server`）
接口 `POST /api/xblue/discover` 接收 `{tenant_id, dataset_ids, doc_ids}`：
1. **收集结构化线索**：读取 `ds_datasets` 与 `ds_dataset_columns`（列名、类型、`distinct_count`、样本值、推测主键）。
2. **收集非结构化线索**：读取指定文档在 `hg_entities` / `hg_edges` 中出现频次最高的前 30 条实体类型与 `RELATES` 谓词模式。
3. **LLM 约束生成**：调用 LLM 输出严格 JSON 草案，包含：
   - 推荐的 `node_types`（含 `properties`、`primary_key`、`agg_rules`、自动布局的 `pos_x`/`pos_y`）
   - 推荐的 `edge_types`（含 `src_type_name`、`dst_type_name`、`cardinality`、`impact_weight`）
   - 推荐的 `mappings`（直接把每个数据集列映射到对应节点属性/指标或关系两端外键）
4. **落库为 `DRAFT`**：写入 `xb_node_types(status='DRAFT')` 与 `xb_data_mappings(status='DRAFT')`，返回前端画布供人工审阅、连线修改后一键 `POST /api/xblue/schema/publish`。

### 5.4 前端可视化设计器规格（`xbluenode-web/src/pages/XBlueStudioPage.tsx`）
引入 `@xyflow/react`（React Flow）构建专业画布：
- **左侧组件面板**：已接入的数据集列表（可展开列字段）、AI 发现模型按钮、新建节点类型按钮。
- **中央模型画布**：
  - 节点卡片渲染：类型名、图标色标、属性列表（区分维度属性 vs 数值指标）、聚合角标（如 `Σ output`）、`DRAFT/PUBLISHED` 状态徽章。
  - 连线交互：从一个节点类型句柄拖拽连向另一节点类型，弹出关系配置面板（关系名、基数 `1:N`/`N:M`、衰减权重 `impact_weight`、传导公式）。
  - 坐标自动持久化：节点拖拽结束防抖保存 `pos_x` / `pos_y`。
- **右侧属性与映射抽屉（Inspector）**：
  - Tab A（Schema 定义）：增删字段属性、配置 `agg_rules`。
  - Tab B（数据映射）：绑定 `ds_datasets`，可视化配置“源列 → 节点主键/标题/属性/指标”映射并支持点击“立即试跑映射（Dry-Run 预览前 10 个生成节点）”。

---

## Step 06：动态聚合引擎与知识-节点同步（Rollup & Sync Engine）

### 6.1 安全表达式求值器（`axpoint-domain/src/expr.rs`）
为支撑 `agg_rules` 中的计算公式以及 `AxSim` 的因果公式，在 `axpoint-domain` 中实现**零 `unsafe`、确定性、无死循环风险的轻量级算术表达式 AST 解析与求值器**：
- 支持字面量（`f64`）、变量引用（`output`、`src.inventory`、`weight`）、四则运算（`+ - * /`）、括号、安全内置函数（`min(a, b)`、`max(a, b)`、`clamp(x, lo, hi)`、`abs(x)`、`coalesce(x, default)`）。
- 除数为零时返回受控数值或 `AppError::BadRequest`，绝不产生 `NaN`/`Infinity` 或 Panic。

### 6.2 映射物化执行器（`Data-to-Node Materializer`）
函数 `materialize_mapping(store: &PgStore, analytics: &AnalyticsEngine, tenant_id: &str, mapping_id: Uuid)`：
1. 从 DuckDB 读取对应 `ds_datasets` 的规范化数据（若有 `filter_sql` 先经 `sql_guard` 校验）。
2. 按 `target_kind` 执行批量事务写入：
   - `NODE`：按 `key_column` 去重合并写入 `xb_nodes`（区分 `attributes` 与 `metrics`），同时向 `xb_metric_series` 写入采样点，向 `xb_provenance` 写入 `DATASET` 血缘。
   - `LINK`：通过 `(src_type_name, src_key)` 与 `(dst_type_name, dst_key)` 解析出 `src_node_id` 与 `dst_node_id`，幂等 `UPSERT` 到 `xb_links`。
3. 触发关联节点类型的动态聚合重算。

### 6.3 跨节点动态聚合引擎（`Rollup Engine`）
当任意一批 `BlueNode`（或 `BlueLink`）发生写入或指标更新时，执行 `recompute_aggregations`：
1. 加载租户下所有带 `agg_rules` 的 `BlueNodeTypeDef` 与 `BlueEdgeTypeDef`。
2. 对每个聚合规则 `{target_metric, op, expr, source_link_type, direction}`（例如：*“中心配送仓的 `downstream_demand` = 所有出向 `FULFILLS` 关联订单节点 `demand` 的 `SUM`”*）：
   - 收集受影响父/子节点的邻居集合；
   - 按 `SUM` / `AVG` / `MAX` / `MIN` / `COUNT` 或 `expr` 计算最新派生指标；
   - 更新目标节点 `xb_nodes.metrics` 并写入 `xb_metric_series(source_tag='AGGREGATION')`。
3. 若存在多层级的聚合依赖（如 订单 → 仓库 → 工厂），使用拓扑序分层推进（带环路访问集合守卫），确保单次变更完成全链路口径对齐。

### 6.4 文档超图实体向业务节点的对齐同步（`Knowledge-to-Node Sync`）
打通非结构化知识与结构化对象：
- 提供 `POST /api/xblue/sync-from-hypergraph`：
  - 将 `hg_entities` 中匹配 `BlueNode.title` 或 `synonyms` 的实体，自动关联其所属段落与 `RELATES` 事实到 `xb_provenance(source_kind='HYPER_ENTITY')`；
  - 支持从段落中通过 LLM 结构化抽取事件指标（如“某工厂因检修停产 3 天，预计产能影响 -40%”）生成待确认的节点状态更新建议（进入人工确认或 `AxSim` 沙盘试算）。

### 6.5 核心 API 契约

| 方法与路径 | 说明 |
| :--- | :--- |
| `POST /api/xblue/discover` | AI 探索数据集与超图生成模型草案（`DRAFT`） |
| `POST /api/xblue/schema/publish` | 人工确认将 `DRAFT` 模型升格为 `PUBLISHED` |
| `GET / POST /api/xblue/mappings` | 列出或创建数据集到节点/关系的映射规则 |
| `POST /api/xblue/mappings/{id}/execute` | 立即执行映射物化并触发全链动态聚合 |
| `POST /api/xblue/aggregations/recompute` | 手动或定时全量重算节点聚合指标 |
| `GET /api/xblue/nodes/{id}/detail` | 获取节点最新指标、时序历史、关联邻居与文档/数据血缘证据 |

### 6.6 Step 05 ~ Step 06 可验证门禁（Verification Gate）

```bash
# 1. 运行安全表达式求值器、拓扑聚合与环路守卫单元测试
psql "$DATABASE_URL" -f deploy/migrations/003b_xbluestudio_mappings.sql
cargo test -p axpoint-domain expr
cargo test -p axpoint-db rollup_and_mapping
cargo clippy --workspace --all-targets -- -D warnings

# 2. 验证完整“数据表 -> 节点映射 -> 动态聚合”端到端链路
curl -s -X POST http://127.0.0.1:9995/api/xblue/mappings/<ORDER_MAPPING_ID>/execute | jq .
curl -s "http://127.0.0.1:9995/api/xblue/nodes/<WAREHOUSE_NODE_ID>/detail" | jq '{metrics, provenance_count: (.provenance | length), series_len: (.series | length)}'
```
**通过标准**：
1. 表达式求值器单测覆盖优先级、括号、除零保护、非法字符拒绝，100% 通过。
2. 执行订单映射写入 `O-1(demand=120)`、`O-2(demand=80)` 后，上游仓库节点 `W-1` 的聚合指标 `downstream_demand` 自动更新为 `200.0`，且 `xb_metric_series` 记录了 `source_tag='AGGREGATION'` 的最新时间点。
3. AI 模型发现接口针对测试数据集输出合法的 `node_types`、`edge_types` 与 `mappings` 草案，在人工调用 `/publish` 前状态严格保持 `DRAFT`，发布后转为 `PUBLISHED`。
4. 前端 `XBlueStudioPage` 画布支持节点拖拽、连线创建关系、保存位置并在刷新后完整恢复坐标。
