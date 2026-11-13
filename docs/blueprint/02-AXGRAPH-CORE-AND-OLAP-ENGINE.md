# 02 · AxGraph 双擎存储底座与 AxAnalytics OLAP 制表引擎（Step 03 ~ Step 04）

本文件定义 **AxGraph 原生双擎底座** 与 **AxAnalytics 结构化 OLAP 分析制表引擎（`axpoint-analytics`）** 的工程实现规范，使平台同时具备“非结构化超图知识推理”和“结构化数据秒级分析、透视制表与智能问数”能力。

---

## Step 03：AxGraph 超图、业务节点与时序指标统一存储底座

### 3.1 设计目标
将非结构化文档超图（`hg_*`）、结构化业务节点实例（`xb_*`）、时序指标变化（`xb_metric_series`）以及数据血缘（`xb_provenance`）统一在 PostgreSQL 16 + `pgvector` 底座中，并补强实体同义词归一化与联合检索能力。

### 3.2 数据库 Schema（`deploy/migrations/003_xblue_timeseries.sql`）

```sql
-- 1. 业务节点类型（含基数约束、数据源绑定与画布坐标）
CREATE TABLE IF NOT EXISTS xb_node_types (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    name         TEXT NOT NULL,
    display_name TEXT NOT NULL,
    icon         TEXT NOT NULL DEFAULT 'Box',
    color        TEXT NOT NULL DEFAULT '#2563eb',
    properties   JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{name, data_type, nullable, is_pii, default_val}]
    agg_rules    JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{target_metric, op, expr, source_link_type, direction}]
    primary_key  TEXT NOT NULL DEFAULT 'node_key',
    pos_x        DOUBLE PRECISION NOT NULL DEFAULT 120.0,
    pos_y        DOUBLE PRECISION NOT NULL DEFAULT 120.0,
    version      INT NOT NULL DEFAULT 1,
    status       TEXT NOT NULL DEFAULT 'PUBLISHED', -- 'DRAFT' | 'PUBLISHED'
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- 2. 业务关系类型（含基数、因果传导公式与方向）
CREATE TABLE IF NOT EXISTS xb_edge_types (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    display_name    TEXT NOT NULL DEFAULT '',
    src_type_name   TEXT NOT NULL,
    dst_type_name   TEXT NOT NULL,
    cardinality     TEXT NOT NULL DEFAULT 'N_TO_M', -- 'ONE_TO_ONE' | 'ONE_TO_MANY' | 'MANY_TO_MANY' | 'N_TO_M'
    impact_weight   DOUBLE PRECISION NOT NULL DEFAULT 0.5,
    impact_formula  TEXT NOT NULL DEFAULT 'src_delta * weight',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name, src_type_name, dst_type_name)
);

-- 3. 业务节点实例（支持维度属性与实时数值指标分离存储，支持全文检索）
CREATE TABLE IF NOT EXISTS xb_nodes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    type_name   TEXT NOT NULL,
    node_key    TEXT NOT NULL,
    title       TEXT NOT NULL,
    attributes  JSONB NOT NULL DEFAULT '{}'::jsonb, -- 非数值维度属性（地区/等级/状态/负责人等）
    metrics     JSONB NOT NULL DEFAULT '{}'::jsonb, -- 最新数值指标快照
    fts         TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('simple', coalesce(title, '') || ' ' || coalesce(node_key, '') || ' ' || coalesce(attributes::text, ''))
    ) STORED,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, type_name, node_key)
);

CREATE INDEX IF NOT EXISTS idx_xb_nodes_type ON xb_nodes (tenant_id, type_name);
CREATE INDEX IF NOT EXISTS idx_xb_nodes_fts ON xb_nodes USING GIN (fts);

-- 4. 业务关系实例
CREATE TABLE IF NOT EXISTS xb_links (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    link_type   TEXT NOT NULL,
    src_node_id UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    dst_node_id UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    weight      DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    properties  JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, link_type, src_node_id, dst_node_id)
);

CREATE INDEX IF NOT EXISTS idx_xb_links_src ON xb_links (tenant_id, src_node_id);
CREATE INDEX IF NOT EXISTS idx_xb_links_dst ON xb_links (tenant_id, dst_node_id);

-- 5. 业务节点时序指标表（支撑指标变化追踪与趋势图）
CREATE TABLE IF NOT EXISTS xb_metric_series (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    node_id     UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    metric_key  TEXT NOT NULL,
    metric_val  DOUBLE PRECISION NOT NULL,
    source_tag  TEXT NOT NULL DEFAULT 'SYNC', -- 'SYNC' | 'AGGREGATION' | 'ACTION' | 'MANUAL'
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_series_lookup
    ON xb_metric_series (tenant_id, node_id, metric_key, recorded_at DESC);

-- 6. 数据与文档血缘追踪表（打通 hg_* 超图文档实体与 xb_* 业务节点）
CREATE TABLE IF NOT EXISTS xb_provenance (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    target_kind     TEXT NOT NULL, -- 'BLUE_NODE' | 'BLUE_LINK'
    target_id       UUID NOT NULL,
    source_kind     TEXT NOT NULL, -- 'DATASET' | 'DOC_PARA' | 'HYPER_ENTITY' | 'FLOW_RUN'
    source_id       UUID NOT NULL,
    source_detail   TEXT NOT NULL DEFAULT '',
    confidence      DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_prov_target ON xb_provenance (tenant_id, target_kind, target_id);
```

### 3.3 Rust 核心检索规范（`axpoint-db`）
1. **同义词与大小写不敏感实体召回**：
   在 `recall_entity_link` 与 `recall_multi_hop` 中，实体匹配条件同时覆盖 `LOWER(ent.name)` 与 `EXISTS (SELECT 1 FROM unnest(ent.synonyms) s WHERE LOWER(s) = ANY($2))`，确保简称、别名、英文大小写均能桥接召回。
2. **混合业务节点检索（`search_blue_nodes`）**：
   同时支持按 `type_name`、全文检索 `fts`、JSONB 属性过滤（如 `attributes->>'region' = '华东'`）以及时序区间查询 `get_node_metric_history(node_id, metric_key, from, to)`。
3. **原子指标写入函数 `record_node_metrics_tx`**：
   每次更新 `xb_nodes.metrics` 时，在同一事务中向 `xb_metric_series` 追加历史采样点，保证当前状态与历史趋势严格一致。

### 3.4 Step 03 可验证门禁（Verification Gate）

```bash
psql "$DATABASE_URL" -f deploy/migrations/003_xblue_timeseries.sql
cargo test -p axpoint-db xblue_timeseries
cargo clippy --workspace --all-targets -- -D warnings
```
**通过标准**：
1. 写入同一业务节点 3 次不同时间点指标后，`xb_nodes.metrics` 保留最新值，`xb_metric_series` 准确按时间倒序保留 3 条历史记录。
2. 插入带 `synonyms = ARRAY['晶圆一厂', 'Fab-1']` 的超图实体后，传入 `"fab-1"` 可同时命中 `EntityLink` 与 `MultiHop` 召回通道。

---

## Step 04：AxAnalytics OLAP 制表分析与智能问数引擎（`axpoint-analytics`）

### 4.1 定位与 Crate 结构
新建 Rust Workspace Crate `axpoint-core/crates/axpoint-analytics`，基于 `duckdb`（启用 `bundled` + `parquet` 特性）提供嵌入式列式计算引擎，所有通过 Step 01/02 注册到 `ds_datasets` 的 Parquet 数据集均可免拷贝直接注册为 DuckDB 虚拟视图进行分析。

```text
axpoint-core/crates/axpoint-analytics/
├── Cargo.toml
└── src/
    ├── lib.rs             # AnalyticsEngine（连接池/只读沙箱/Parquet 视图挂载）
    ├── sql_guard.rs       # 基于 sqlparser 的 AST 安全守卫（仅允许 SELECT/WITH，禁止 DDL/DML/文件函数）
    ├── pivot.rs           # 动态透视表构建器（行维度/列维度/指标聚合/小计总计）
    ├── chart_advisor.rs   # 基于返回结果集列类型自动推荐可视化图表类型（Table/Line/Bar/Pie/Scatter）
    └── nl2sql.rs          # 智能问数管线（Schema Prompt 组装 -> LLM 生成 SQL -> AST 校验 -> 执行 -> 纠错重试）
```

### 4.2 Rust 核心类型与 Trait 契约（零 `unsafe` / 零 `as` / 零 `unwrap`）

```rust
/// 结构化分析查询请求（用于制表与透视分析）。
#[derive(Debug, Clone, Deserialize)]
pub struct TableAnalysisRequest {
    /// 租户标识。
    pub tenant_id: String,
    /// 参与分析的数据集 ID 列表。
    pub dataset_ids: Vec<Uuid>,
    /// 分组维度字段列表。
    pub dimensions: Vec<String>,
    /// 透视列字段（可选，若提供则生成二维交叉透视表）。
    pub pivot_column: Option<String>,
    /// 聚合指标列表（如 `SUM(output) AS total_output`）。
    pub measures: Vec<MeasureSpec>,
    /// 过滤条件（结构化谓词树，防止 SQL 注入）。
    pub filters: Vec<FilterPredicate>,
    /// 排序规则。
    pub sort_by: Vec<SortSpec>,
    /// 分页大小与偏移。
    pub limit: usize,
    /// 分页偏移量。
    pub offset: usize,
}

/// 聚合指标定义。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MeasureSpec {
    /// 目标列名。
    pub column: String,
    /// 聚合函数：`SUM` | `AVG` | `COUNT` | `COUNT_DISTINCT` | `MIN` | `MAX`。
    pub agg: String,
    /// 输出别名。
    pub alias: String,
}

/// 统一表格查询结果（供前端制表、图表渲染与出品中心复用）。
#[derive(Debug, Clone, Serialize)]
pub struct TabularResult {
    /// 输出列元信息。
    pub columns: Vec<ResultColumnMeta>,
    /// 二维行数据（JSON 值）。
    pub rows: Vec<Vec<serde_json::Value>>,
    /// 命中的总行数（用于分页）。
    pub total_rows: usize,
    /// 实际执行的只读 SQL（便于透明溯源）。
    pub executed_sql: String,
    /// 自动推荐图表配置。
    pub recommended_chart: ChartSuggestion,
    /// 执行耗时（毫秒）。
    pub elapsed_ms: u128,
}
```

### 4.3 SQL AST 安全守卫（`sql_guard.rs`）
无论来自前端高级 SQL 编辑器还是 LLM `Text-to-SQL`，所有 SQL 必须在进入 DuckDB 前经过 `sqlparser` AST 解析校验：
1. 顶层语句必须且仅能是单个 `Statement::Query`（`SELECT` / `WITH ... SELECT`）。
2. 递归遍历 AST，**显式拒绝**以下高危函数与语法：`read_csv`、`read_parquet`、`read_json`、`copy`、`attach`、`install`、`load`、`pragma`、`getenv`（所有数据集只能通过系统预先安全注册的视图名 `ds_<safe_name>` 访问）。
3. 强制外层包裹或注入 `LIMIT` 上限（默认最大 `10_000` 行，导出任务走独立流式通道）。
4. 在 `nl2sql.rs` 中若 DuckDB 执行报语法/列名错误，自动将错误信息与真实 Schema 反馈给 LLM 最多重试 2 次（Self-Correction Loop）。

### 4.4 智能融合问答路由（`AxQuery` 双模态融合路由）
在 `POST /api/chat/query` 中引入意图分类器（Intent Router）：
- **DOC_RAG（知识推理型）**：走五路超图检索 + RRF 融合回答。
- **TABULAR_ANALYTICS（数值问数/制表型）**：走 `nl2sql.rs` 在 DuckDB 中运行聚合查询，返回 `TabularResult`（前端渲染交互表格 + 图表）及自然语言总结。
- **HYBRID（复合决策型）**：同时执行超图政策/规则检索 + DuckDB 指标查询，合并上下文合成回答。

### 4.5 核心 API 契约（`axpoint-server` `/api/analytics/*`）

| 方法与路径 | 说明 | 请求体 | 响应体 |
| :--- | :--- | :--- | :--- |
| `POST /api/analytics/pivot` | 执行维度/指标/透视制表分析 | `TableAnalysisRequest` | `TabularResult` |
| `POST /api/analytics/sql` | 执行用户编写的受控只读 SQL | `{tenant_id, dataset_ids, sql, limit}` | `TabularResult` |
| `POST /api/analytics/ask` | 自然语言智能问数（Text-to-SQL + 推荐图表） | `{tenant_id, dataset_ids, question}` | `{answer_summary, sql, result: TabularResult}` |
| `POST /api/analytics/save-view` | 将分析 SQL 物化保存为派生数据集（供后续建模/看板复用） | `{tenant_id, name, display_name, dataset_ids, sql}` | `DatasetDetail` |

### 4.6 Step 04 可验证门禁（Verification Gate）

```bash
# 1. 运行 DuckDB 分析、透视表与 AST 安全守卫单测
cargo test -p axpoint-analytics
cargo clippy --workspace --all-targets -- -D warnings

# 2. 验证恶意 SQL 拦截（必须返回 400 Bad Request）
curl -i -s -X POST http://127.0.0.1:9995/api/analytics/sql \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"default","dataset_ids":[],"sql":"SELECT * FROM read_csv('\''/etc/passwd'\'')","limit":10}'

# 3. 验证透视制表与自然语言问数
curl -s -X POST http://127.0.0.1:9995/api/analytics/pivot \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"default","dataset_ids":["<DATASET_UUID>"],"dimensions":["region"],"measures":[{"column":"output","agg":"SUM","alias":"total_output"}],"filters":[],"sort_by":[],"limit":100,"offset":0}' | jq '{total_rows, recommended_chart, rows: .rows[0:3]}'
```
**通过标准**：
1. `sql_guard` 对 `DROP`、`INSERT`、`read_csv`、`PRAGMA`、多语句 `;` 注入 100% 拦截并返回 `400 Bad Request`。
2. 跨两个 Parquet 数据集执行 `JOIN + GROUP BY` 透视计算耗时 `< 200ms`（10 万行测试集）。
3. `POST /api/analytics/ask` 针对测试问题生成合法 DuckDB SQL，返回正确数值表格与匹配的图表建议（如趋势问题推荐 `LINE`，分类对比推荐 `BAR`）。
