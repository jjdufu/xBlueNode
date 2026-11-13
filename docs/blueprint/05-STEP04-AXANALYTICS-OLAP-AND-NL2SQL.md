# 05 · Step 04：AxAnalytics OLAP、透视制表、指标语义层与智能问数

AxAnalytics 在 Core 内以 `duckdb` crate 读取 DuckLake 中的数据集，并以只读方式联邦查询平台对象（业务节点、关系、时序指标）与标记为“直连”的外部关系库。所有 SQL —— 无论用户手写、透视生成、指标编译还是大模型生成 —— 都经过同一个 `sql_guard`：AST 白名单校验 + 标识符改写 + 结果上限。

---

## 04.0 目标、前置与契约

- **目标**：安全的 SQL 运行与解释；拖拽式透视表（含小计、总计、行列上限）；指标语义层（定义、认证、按维度与时间查询）；智能问数（意图路由、模式链接、生成、守卫、试运行、自修复、答案摘要、多轮上下文、反馈学习）；AI 数据处理（自然语言 → 变换 SQL 草案 → 试运行预览 → 确认后物化为派生数据集）；AxQuery 混合问答（文档 + 数值）。
- **前置**：Step 02、Step 03 通过。
- **契约清单**：`0005_analytics_semantic.sql`；`axpoint-analytics` 公开类型（`QueryRequest`、`QueryResult`、`PivotSpec`、`PivotResult`、`MetricDef`、`MetricQuery`、`AskRequest`、`AskResponse`、`TransformDraft`）；端口 `AnalyticsQueryPort`（Step 07 / 09 / 11 消费）；`docs/API_CONTRACT.md` 的 `/api/analytics/*` 与 `/api/chat/query` 扩展；`lib/api/analytics.ts`；Sidecar `/lake/materialize`。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0005_analytics_semantic.sql` | §04.1 |
| 新建 | `axpoint-core/crates/axpoint-analytics/{Cargo.toml, src/lib.rs}` | Crate 入口 |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/session.rs` | 租户 DuckDB 实例池（§04.2） |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/sql_guard.rs` | AST 白名单与改写（§04.3） |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/result.rs` | Arrow → JSON、脱敏、截断 |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/pivot.rs` | 透视编译与矩阵化（§04.4） |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/semantic.rs` | 指标语义层（§04.5） |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/nl2sql/{mod.rs, intent.rs, schema_link.rs, prompt.rs, repair.rs, summarize.rs}` | 智能问数（§04.6） |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/transform.rs` | AI 数据处理（§04.7） |
| 新建 | `axpoint-core/crates/axpoint-analytics/tests/{guard.rs, pivot.rs, semantic.rs, nl2sql_eval.rs}`、`tests/nl2sql_eval/cases.yaml`、`tests/nl2sql_eval/cassette.jsonl` | 测试与评测集 |
| 修改 | `axpoint-core/Cargo.toml` | 成员 `axpoint-analytics`；依赖见 A6 Step 04 |
| 修改 | `deploy/docker/axpoint-core.Dockerfile` | 构建阶段用与 `duckdb` crate 同版本的 DuckDB CLI 执行 `INSTALL ducklake; INSTALL postgres; INSTALL mysql_scanner; INSTALL sqlite_scanner;`，把 `~/.duckdb/extensions` 复制到运行镜像 `/home/nonroot/.duckdb/extensions` |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/analytics.rs` | `/api/analytics/*` |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/documents.rs` | `/api/chat/query` 意图路由（§04.8） |
| 修改 | `axpoint-parser/lake/writer.py`、`axpoint-parser/main.py` | `POST /lake/materialize` |
| 新建 | `xbluenode-web/src/pages/AnalyticsStudioPage.tsx`、`src/components/analytics/{SqlEditor.tsx, SchemaTree.tsx, PivotBuilder.tsx, ResultGrid.tsx, ChartPanel.tsx, MetricManager.tsx, TransformWizard.tsx}`、`src/lib/api/analytics.ts` | 前端 |
| 修改 | `xbluenode-web/src/pages/AxQueryPage.tsx` | 意图标识、SQL 与结果面板、引用面板、反馈按钮 |
| 新建 | `scripts/bench/{analytics-data.sh, analytics-bench.sh}` | 基准数据（1 000 万行）与基准断言 |
| 新建 | `scripts/seed/04-analytics.sh` | 演示指标、保存查询 |
| 新建 | `scripts/gate/step04.sh` | 门禁 |

---

## 04.1 迁移 `deploy/migrations/0005_analytics_semantic.sql`

```sql
-- 0005_analytics_semantic.sql：指标语义层、保存查询、查询历史、问数会话、变换草案

CREATE TABLE IF NOT EXISTS an_metrics (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    name           TEXT NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    display_name   TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    dataset_id     UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE RESTRICT,
    expression     TEXT NOT NULL,                 -- 聚合表达式，例 SUM(amount) / NULLIF(COUNT(DISTINCT order_id), 0)
    filter_sql     TEXT NOT NULL DEFAULT '',      -- 固定过滤条件（布尔表达式）
    time_column    TEXT,
    default_grain  TEXT NOT NULL DEFAULT 'DAY' CHECK (default_grain IN ('MINUTE','HOUR','DAY','MONTH')),
    dimensions     JSONB NOT NULL DEFAULT '[]'::jsonb,   -- [{name, column, display_name}]
    unit           TEXT NOT NULL DEFAULT '',
    format         TEXT NOT NULL DEFAULT '',             -- 例 "0,0.00"、"0.0%"
    synonyms       TEXT[] NOT NULL DEFAULT '{}',
    certified      BOOLEAN NOT NULL DEFAULT FALSE,
    schema_version INT NOT NULL,                         -- 定义时数据集的 schema_version
    owner_user_id  UUID,
    version        INT NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);
ALTER TABLE an_metrics ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(name,'') || ' ' || coalesce(display_name,'') || ' ' || coalesce(description,'')
                || ' ' || array_to_string(synonyms, ' '))) STORED;
CREATE INDEX IF NOT EXISTS idx_an_metrics_fts ON an_metrics USING GIN (fts);

CREATE TABLE IF NOT EXISTS an_saved_queries (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('SQL','PIVOT','METRIC','ASK')),
    spec          JSONB NOT NULL,                  -- {sql} | PivotSpec | MetricQuery | {question, sql}
    parameters    JSONB NOT NULL DEFAULT '[]'::jsonb,   -- [{name, data_type, default}]，SQL 中以 :name 引用
    owner_user_id UUID,
    shared        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS an_query_history (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    user_id     UUID,
    kind        TEXT NOT NULL CHECK (kind IN ('SQL','PIVOT','METRIC','ASK','TRANSFORM')),
    sql_text    TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('SUCCEEDED','FAILED','REJECTED','TIMEOUT')),
    row_count   BIGINT,
    duration_ms BIGINT,
    error_code  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_an_query_history ON an_query_history (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS an_ask_turns (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    session_id     UUID NOT NULL,
    user_id        UUID,
    question       TEXT NOT NULL,
    intent         TEXT NOT NULL CHECK (intent IN ('DOC','DATA','HYBRID','CHITCHAT')),
    linked_objects JSONB NOT NULL DEFAULT '{}'::jsonb,  -- {datasets, columns, metrics, nodes}
    sql_text       TEXT,
    result_preview JSONB,
    answer         TEXT NOT NULL DEFAULT '',
    citations      JSONB NOT NULL DEFAULT '[]'::jsonb,
    feedback       TEXT CHECK (feedback IN ('UP','DOWN')),
    corrected_sql  TEXT,
    model_name     TEXT NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_an_ask_turns_session ON an_ask_turns (tenant_id, session_id, created_at);
ALTER TABLE an_ask_turns ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(question,''))) STORED;
CREATE INDEX IF NOT EXISTS idx_an_ask_turns_fts ON an_ask_turns USING GIN (fts);

CREATE TABLE IF NOT EXISTS an_transform_drafts (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           TEXT NOT NULL,
    user_id             UUID,
    instruction         TEXT NOT NULL,
    source_dataset_ids  UUID[] NOT NULL,
    sql_text            TEXT NOT NULL,
    preview             JSONB NOT NULL DEFAULT '{}'::jsonb,   -- {columns, rows(≤100), row_count_estimate}
    target_dataset_name TEXT NOT NULL CHECK (target_dataset_name ~ '^[a-z][a-z0-9_]{0,62}$'),
    status              TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','APPLIED','DISCARDED')),
    dataset_id          UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'an_metrics','an_saved_queries','an_query_history','an_ask_turns','an_transform_drafts'
]::regclass[]) AS t;
```

---

## 04.2 租户 DuckDB 实例池（`session.rs`）

```rust
/// 每租户一个内存 DuckDB 实例；LRU 上限 AXPOINT_DUCKDB_TENANT_CACHE（默认 32）。
pub struct SessionPool {
    inner: tokio::sync::Mutex<lru::LruCache<String, std::sync::Arc<TenantDb>>>,
    cfg: AnalyticsConfig,
}

/// 已加固的租户实例。
pub struct TenantDb {
    db: duckdb::Connection,          // 根连接，仅用于 try_clone()
    tenant_id: String,
    built_at: std::time::Instant,
    catalog: TenantCatalog,          // 数据集名 → 湖表、列与 PII 信息；直连连接别名
}
```

**构建顺序**（`TenantDb::build`，顺序不可调换）：

1. `Connection::open_in_memory_with_flags(Config::default().enable_autoload_extension(false)? .allow_unsigned_extensions(false)?)`。
2. `LOAD ducklake; LOAD postgres; LOAD mysql_scanner; LOAD sqlite_scanner;`（扩展随镜像预装在 `$HOME/.duckdb/extensions`，见 A6）。
3. 湖：`ATTACH 'ducklake:postgres:<lake libpq>' AS lake (DATA_PATH '<data_root>/lake/', METADATA_SCHEMA 'lake_catalog', READ_ONLY)`。
4. 平台对象：`ATTACH 'host=... dbname=xbluenode user=axpoint_app password=... options=''-c app.tenant_id=<tenant_id>''' AS ax_pg (TYPE postgres, READ_ONLY)`。连接启动参数设置租户 GUC，PostgreSQL 行级安全据此只返回该租户数据。
5. 平台视图（`CREATE VIEW` 在 DuckDB 内存库的 `ax` 模式中）：

| 视图 | 定义 |
| :--- | :--- |
| `ax.nodes` | `SELECT id, type_name, node_key, title, properties, metrics, geo_lng, geo_lat, updated_at FROM ax_pg.public.xb_nodes` |
| `ax.links` | `SELECT id, link_type, src_node_id, dst_node_id, weight, properties FROM ax_pg.public.xb_links` |
| `ax.metric_points` | `SELECT node_id, metric, ts, value, value_text, source_tag FROM ax_pg.public.xb_metric_series` |
| `ax.metric_rollups` | `SELECT node_id, metric, grain, bucket_start, cnt, sum_v, min_v, max_v, last_v FROM ax_pg.public.xb_metric_rollups` |

6. 直连：租户内 `config.live_query = true` 的 `POSTGRES` / `MYSQL` / `SQLITE` 连接，以 `ATTACH ... AS live_<连接名> (TYPE postgres|mysql|sqlite, READ_ONLY)` 挂载（凭证在 Core 内解密）。
7. 加固：`SET enable_external_access = false; SET threads = 4; SET memory_limit = '<AXPOINT_DUCKDB_MEMORY>'; SET max_temp_directory_size = '10GB'; SET lock_configuration = true;`。
8. 构建 `TenantCatalog`：读 `ds_datasets`（`status='READY'` 或已有快照）与 `ds_column_meta`。

**失效**：订阅 `dataset.synced` 外发事件只刷新 `TenantCatalog`；连接增删改、`live_query` 变化、`AXPOINT_LAKE_CATALOG_DSN` 变化时从池中移除该租户实例（下次查询重建）。

**执行**：每次查询 `db.try_clone()` 得到新连接，在 `tokio::task::spawn_blocking` 中执行；超时（默认 30 秒，`QueryRequest.timeout_ms` 上限 120 000）时调用 `InterruptHandle::interrupt()` 并返回 504 `QUERY_TIMEOUT`。

---

## 04.3 SQL 守卫（`sql_guard.rs`）

```rust
/// 校验并改写 SQL。
///
/// # Errors
/// 违反任一规则时返回 `AppError::BadRequest("SQL_REJECTED", 原因)`。
pub fn guard(sql: &str, catalog: &TenantCatalog, opts: &GuardOptions) -> Result<GuardedSql, AppError>;

pub struct GuardOptions {
    pub max_rows: u32,                 // 默认 10 000，上限 100 000
    pub allow_live: bool,              // 是否允许引用 live_* 直连
    pub params: BTreeMap<String, serde_json::Value>,  // :name 参数
}

pub struct GuardedSql {
    pub sql: String,                   // 改写后的 SQL（已包装 LIMIT max_rows + 1）
    pub referenced_datasets: Vec<Uuid>,
    pub referenced_columns: Vec<(Uuid, String)>,
    pub uses_platform_views: bool,
}
```

规则（解析器 `sqlparser::dialect::DuckDbDialect`）：

| # | 规则 |
| :--- | :--- |
| 1 | 恰好一条语句；`Statement::Query` 之外一律拒绝（含 `INSERT/UPDATE/DELETE/MERGE/CREATE/DROP/ALTER/COPY/ATTACH/DETACH/INSTALL/LOAD/SET/RESET/PRAGMA/CALL/EXPORT/IMPORT/USE/CHECKPOINT/VACUUM`） |
| 2 | 表引用解析：一段名 `orders` → 当前租户数据集 `lake."tenant_<id>"."orders"`；两段名 `ax.nodes` → 平台视图（只允许上表 4 个）；三段名 `live_<conn>.<schema>.<table>` → 直连（`allow_live=true` 且连接存在）；其余（含 `lake.*`、`ax_pg.*`、`main.*`、`system.*`、`information_schema.*`、其他租户模式）拒绝 |
| 3 | 表函数只允许 `range`、`generate_series`、`unnest`；其他表函数（`read_csv*`、`read_parquet`、`read_json*`、`*_scan`、`glob`、`query`、`query_table`、`ducklake_*`、`postgres_query`、`iceberg_*`、`delta_*`）拒绝 |
| 4 | 标量函数黑名单：`getenv`、`current_setting`、`read_text`、`read_blob`、`pg_*`、`duckdb_*`、`pragma_*`、`load_extension`、`sql_auto_complete`；以及任何以 `__internal` 开头的函数 |
| 5 | CTE 名在作用域内遮蔽数据集名（按 SQL 语义处理），CTE 引用不做改写 |
| 6 | `:name` 参数替换为带类型的字面量（字符串用 `'...'` 转义单引号，数值与布尔校验后输出），未提供的参数报错 |
| 7 | 输出包装：`SELECT * FROM (<改写后 SQL>) AS q LIMIT <max_rows + 1>`；返回行数 > `max_rows` 时截断并设 `truncated=true` |
| 8 | 改写后再解析一次，确认规则 1～4 仍成立（防止改写引入问题） |

**测试**（`tests/guard.rs`，表驱动 ≥ 60 条）：每条拒绝规则至少 3 个变体（大小写、引号包裹、注释夹带、嵌套子查询中、CTE 中、`UNION` 分支中）；改写正确性（JOIN、子查询、CTE 遮蔽、别名）；`SELECT * FROM read_csv('/etc/passwd')`、`SELECT getenv('HOME')`、`ATTACH ':memory:' AS x`、`SELECT * FROM lake."tenant_other".orders`、`SELECT * FROM ax_pg.public.sec_users` 全部拒绝。

**结果处理**（`result.rs`）：

- Arrow → JSON 按 A1 §7（`DECIMAL` 与超大整数为字符串，时间为 RFC 3339）。
- 列来源追踪：守卫记录每个输出列的来源 `(dataset, column)`（直接引用时可追踪；表达式列标记为派生）。来源列 `pii_category ≠ NONE` 且主体无 `PII_RAW_READ` 时按 A1 `MaskRule` 脱敏；派生自 PII 列的表达式列整体 `REDACT`。完整的列级策略在 Step 13 接入同一入口。
- 每次执行写 `an_query_history` 与审计 `QUERY_SQL`（SQL 文本截断 4 000 字符）。

---

## 04.4 透视制表（`pivot.rs`）

```rust
pub struct PivotSpec {
    pub dataset: String,                       // 数据集名，或 "ax.metric_rollups" 等平台视图
    pub rows: Vec<FieldRef>,                   // 行维度（≤ 5）
    pub columns: Vec<FieldRef>,                // 列维度（≤ 3）
    pub values: Vec<ValueSpec>,                // {field, agg: AggFn, alias, format}（≤ 10）
    pub filters: Vec<FilterSpec>,              // {field, op, value}
    pub time_bucket: Option<TimeBucket>,       // {field, grain}
    pub sort: Vec<SortSpec>,
    pub subtotals: bool,
    pub grand_total: bool,
    pub row_limit: u32,                        // ≤ 10 000
    pub column_limit: u32,                     // 透视后列组合 ≤ 200
}

pub enum FilterOp { Eq, Ne, Gt, Gte, Lt, Lte, In, NotIn, Between, Like, IsNull, IsNotNull }
```

编译规则：

1. 字段名经数据集目录校验后以双引号引用；过滤值全部生成参数字面量（复用守卫规则 6）。
2. `time_bucket` → `date_trunc('<grain>', "<field>")`。
3. `subtotals=true` → `GROUP BY GROUPING SETS ((r1..rn, c1..cm), (r1..rn-1, c1..cm), ..., ())`，并输出 `GROUPING(...)` 位标识区分小计行；`grand_total=true` 时包含 `()` 组。
4. 生成的 SQL 经过 `guard()`，与手写 SQL 走同一执行链路。
5. 矩阵化在 Rust 中完成：列维度组合按出现频次取前 `column_limit` 个，其余合并为“其他”（`agg` 为 `SUM/COUNT/MIN/MAX` 时可合并，`AVG/MEDIAN/COUNT_DISTINCT` 时不合并并设 `columns_truncated=true`）。

响应（`row_kinds` 取值 `DATA` / `SUBTOTAL` / `GRAND_TOTAL`）：

```json
{
  "row_headers": [["华东", "上海"], ["华东", "小计"]],
  "column_headers": [["2026-01", "金额"], ["2026-02", "金额"]],
  "cells": [["12000.50", "13010.00"], ["25010.50", "27020.00"]],
  "row_kinds": ["DATA", "SUBTOTAL"],
  "columns_truncated": false,
  "sql": "SELECT ...",
  "duration_ms": 182
}
```

**测试**（`tests/pivot.rs`）：用 3 × 4 的小数据集逐格比对小计、总计；`AVG` 不被错误合并；`column_limit` 截断标志。

---

## 04.5 指标语义层（`semantic.rs`）

### 04.5.1 定义校验

`POST /api/analytics/metrics`（`STUDIO_EDIT`）：

1. `expression` 必须能被解析为单个表达式且包含至少一个聚合函数（`SUM/AVG/COUNT/MIN/MAX/MEDIAN/approx_count_distinct/quantile_cont`），且引用的列都在数据集中；不允许子查询。
2. `filter_sql` 必须是布尔表达式，同样只引用本数据集列。
3. `dimensions[].column` 必须存在；`time_column` 的 `DataType` 必须是 `DATE` 或 `TIMESTAMP`。
4. 记录 `schema_version`；数据集后续 Schema 变化导致引用列消失时，指标查询返回 409 `SCHEMA_BREAKING_CHANGE`，指标列表标注 `broken=true`。
5. 定义变化 `version + 1`；`certified=true` 只能由 `STUDIO_PUBLISH` 设置。
6. 写血缘 `ds_lineage(DATASET → METRIC)`。

### 04.5.2 查询编译

`POST /api/analytics/metrics/{name}/query`：

```json
{"dimensions": ["region"], "filters": [{"field": "status", "op": "Eq", "value": "PAID"}],
 "time_range": {"from": "2026-01-01T00:00:00Z", "to": "2026-03-01T00:00:00Z"}, "grain": "MONTH",
 "compare": "PREVIOUS_PERIOD"}
```

编译为：

```sql
SELECT date_trunc('month', "<time_column>") AS period, "region", <expression> AS value
FROM <dataset>
WHERE (<filter_sql>) AND "<time_column>" >= :from AND "<time_column>" < :to AND "status" = :f0
GROUP BY ALL ORDER BY period
```

`compare=PREVIOUS_PERIOD | SAME_PERIOD_LAST_YEAR` 时再执行一次平移区间的查询，按维度对齐后返回 `value`、`compare_value`、`delta`、`delta_ratio`（分母为 0 时 `delta_ratio=null`）。

多个指标可在 `POST /api/analytics/metrics/query-multi` 中共享维度一起查询（同一数据集合并为一条 SQL，不同数据集分别查询后在 Rust 中按维度外连接）。

---

## 04.6 智能问数（`nl2sql/`）

### 04.6.1 管线

```text
问题 + 会话上下文（同一 session_id 最近 6 轮的问题、SQL、结果列）
 → ① 意图路由 intent.rs
 → ② 模式链接 schema_link.rs
 → ③ 提示构造 prompt.rs → 大模型（axpoint-llm，JSON 输出）
 → ④ 守卫 guard() → 试运行 EXPLAIN
 → ⑤ 失败时自修复 repair.rs（最多 2 轮）
 → ⑥ 执行（max_rows=1 000）→ 脱敏
 → ⑦ 答案摘要 summarize.rs + 图表建议
 → 写 an_ask_turns、an_query_history、审计 ASK_DATA
```

### 04.6.2 ① 意图路由

1. 规则：问题含数值意图词（`多少|总和|合计|平均|最大|最小|排名|top|同比|环比|占比|趋势|分布|按.*统计|count|sum|avg`）且模式链接命中至少一个数据集列或指标 → `DATA`；问题含“为什么 / 依据 / 制度 / 规定 / 文档 / 原文”且无数值意图 → `DOC`；两者都有 → `HYBRID`。
2. 规则无法判定时调用大模型分类（输出 `{intent, confidence}`，温度 0），`confidence < 0.6` 时取 `HYBRID`。
3. `CHITCHAT` 直接由大模型回答，不访问数据。

### 04.6.3 ② 模式链接

- 候选召回：问题分词后在 `ds_datasets.fts`、`ds_column_meta`（列名、描述、业务术语）、`ds_glossary_terms`（同义词展开）、`an_metrics.fts` 中检索；另对列描述与指标描述的向量（入库时由 `EMBED_NODES` 同一管线生成，`hg_embeddings.node_type` 取 `DS_COLUMN` / `AN_METRIC`；列没有独立 UUID，`node_id = UUIDv5(dataset_id, column_name)`）做相似度检索。
- 打分：`0.5 × FTS 排名分 + 0.5 × 向量相似度`，指标 `certified=true` 加 0.1。取前 5 个数据集、每个数据集前 15 列、前 5 个指标。
- 节点：问题中的实体名经 Step 03 归一化匹配 `xb_nodes`（命中时把节点键作为过滤值候选写入提示）。

### 04.6.4 ③ 提示

系统提示固定模板（`prompt.rs` 中的常量，随代码评审）：

```text
你是 DuckDB SQL 专家。只能使用下列表与列，只输出一个 JSON 对象：{"sql": "...", "explanation": "...", "used_metrics": []}
规则：只写单条 SELECT；表名直接使用给出的名称；指标必须使用给出的表达式；时间条件使用半开区间；
不确定时在 explanation 中说明需要澄清的问题，并令 sql 为空字符串。
当前时间：{now_rfc3339}（租户时区 {tz}）
```

用户提示包含：候选表的 DDL（`CREATE TABLE name(col TYPE -- 描述 / 业务术语 / 单位)`）、每表 3 行脱敏样例、候选指标（名称、表达式、过滤、维度）、最多 5 条少样本（来自同租户 `an_ask_turns` 中 `feedback='UP'` 或有 `corrected_sql` 的记录，按问题 FTS 相似度选取）、会话上下文、当前问题。

### 04.6.5 ④⑤ 守卫、试运行与自修复

- `sql` 为空 → 返回 `need_clarification=true` 与 `explanation`。
- 守卫失败或 `EXPLAIN` 失败 → 把错误信息（`SQL_REJECTED` 原因或 DuckDB 错误首行）回填到提示中请求修正，最多 2 轮；仍失败返回 `status=FAILED`，不执行。

### 04.6.6 ⑦ 答案摘要

- 结果 ≤ 20 行时把结果表交给大模型生成 1～3 句中文结论，**数值必须逐字来自结果表**：摘要生成后用正则提取其中所有数字，逐个在结果单元格中查找（允许千分位与百分号格式差异），找不到的数字导致摘要被替换为模板句“查询返回 N 行，主要结果见表格”。
- 图表建议：1 个时间列 + 数值列 → `LINE_CHART`；1 个分类列 + 1 个数值列且行数 ≤ 12 → `BAR_CHART`（占比类问题 → `PIE_CHART`）；单行单值 → `KPI_CARD`；其余 → `TABLE`。

### 04.6.7 接口

| 方法与路径 | 权限 | 说明 |
| :--- | :--- | :--- |
| `POST /api/analytics/ask` | `DATA_READ` | `{question, session_id?, context?}`（`context = {dataset_ids?, filters?, drill_path?, dashboard_id?}`：限定候选数据集并把筛选条件作为附加约束写入提示，Step 11 运行页问数使用）→ `AskResponse {turn_id, session_id, intent, sql, columns, rows, truncated, answer, chart, need_clarification, linked_objects}` |
| `POST /api/analytics/ask/{turn_id}/feedback` | `DATA_READ` | `{feedback: "UP"|"DOWN", corrected_sql?}`；`corrected_sql` 经守卫校验后保存 |
| `GET /api/analytics/ask/sessions/{session_id}` | `DATA_READ` | 会话全部轮次 |

### 04.6.8 评测

- `tests/nl2sql_eval/cases.yaml`：60 条用例覆盖单表聚合、多表关联、时间过滤、同比环比、TopN、指标引用、节点过滤、多轮指代（“那华南呢？”）。每条含 `question`、`session`（前序问题）、`expected_result`（对演示数据执行参考 SQL 得到的结果集，比较时忽略列名与行序）。
- CI：`cargo test -p axpoint-analytics nl2sql_eval` 使用 `cassette.jsonl` 中录制的模型响应（按提示 SHA-256 查表），断言执行准确率 ≥ 80%；提示模板或模式链接逻辑变化时，用 `AXPOINT_EVAL_RECORD=1` 连接真实模型重新录制并在同一 PR 提交。
- 真实模型评测：`AXPOINT_EVAL_LIVE=1 cargo test -p axpoint-analytics nl2sql_eval -- --nocapture` 打印准确率、平均修复轮次、平均延迟，用于模型选型（Step 09 多模型路由按此结果配置默认问数模型）。

---

## 04.7 AI 数据处理（`transform.rs`）

| 步骤 | 接口 | 行为 |
| :--- | :--- | :--- |
| 草案 | `POST /api/analytics/transform/draft`（`DATA_SYNC`） | `{instruction, source_datasets: [...], target_dataset_name}`；大模型按“清洗 / 去重 / 类型转换 / 拆分列 / 关联 / 聚合 / 派生列”生成单条 `SELECT`；经守卫；`EXPLAIN` + 执行前 100 行作为预览；与源数据集比较输出列差异；写 `an_transform_drafts(DRAFT)` |
| 修改 | `PATCH /api/analytics/transform/{id}` | 用户直接编辑 `sql_text`，重新守卫与预览 |
| 应用 | `POST /api/analytics/transform/{id}/apply` | 创建数据集（`source_kind=DERIVED_VIEW`、`sync_mode=SNAPSHOT`、`source_ref={sql: 改写后 SQL, depends_on: [dataset_id...]}`），入队 `SYNC_DATASET`；`status=APPLIED`；写血缘 `DATASET → DATASET`（`via=transform:<draft_id>`） |
| 丢弃 | `POST /api/analytics/transform/{id}/discard` | `status=DISCARDED` |

`DERIVED_VIEW` 数据集的同步由 Sidecar `POST /lake/materialize {tenant_id, lake_table, sql, batch_id}` 执行：在写连接中 `BEGIN; CREATE OR REPLACE TABLE lake."tenant_<id>".<t> AS <sql>; COMMIT;`。Sidecar 侧用 `sqlglot` 再次校验 SQL 只引用 `lake."tenant_<id>".*`，写连接同样 `enable_external_access=false`。上游数据集同步成功（`dataset.synced`）后，依赖它的派生数据集自动入队同步（按 `depends_on` 拓扑序，检测到环时拒绝创建）。

---

## 04.8 AxQuery 意图路由与混合问答

`POST /api/chat/query` 请求体增加 `session_id?`、`mode?: "AUTO"|"DOC"|"DATA"`（默认 `AUTO`）：

| 意图 | 处理 | 响应字段 |
| :--- | :--- | :--- |
| `DOC` | 现有五路召回 + RRF + 大模型回答 | `answer`、`citations`（段落 ID、文档标题、片段） |
| `DATA` | §04.6 管线 | `answer`、`sql`、`columns`、`rows`、`chart` |
| `HYBRID` | 并行执行两条管线；把数值结果表与召回段落一起交给大模型生成回答；回答中数值同样经 §04.6.6 校验 | 两组字段都返回，`answer` 中以 `[文n]` 与 `[表]` 标注来源 |

召回命中的实体若链接了业务节点（Step 03 `xb_entity_links`），响应附加 `related_nodes: [{id, type_name, title, metrics}]`。

---

## 04.9 Core 接口汇总 `/api/analytics/*`

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /catalog`（数据集、列、平台视图、直连对象树） | `DATA_READ` |
| `POST /sql/run`（`{sql, params?, max_rows?, timeout_ms?, allow_live?}`） | `DATA_READ` |
| `POST /sql/explain` | `DATA_READ` |
| `POST /pivot` | `DATA_READ` |
| `GET /metrics`、`POST /metrics`、`GET /metrics/{name}`、`PATCH /metrics/{name}`、`DELETE /metrics/{name}` | `DATA_READ` / `STUDIO_EDIT` |
| `POST /metrics/{name}/query`、`POST /metrics/query-multi` | `DATA_READ` |
| `GET /saved-queries`、`POST /saved-queries`、`GET /saved-queries/{id}`、`PATCH /saved-queries/{id}`、`DELETE /saved-queries/{id}`、`POST /saved-queries/{id}/run` | `DATA_READ` / `VIEW_EDIT` |
| `POST /ask`、`POST /ask/{turn_id}/feedback`、`GET /ask/sessions/{session_id}` | `DATA_READ` |
| `POST /transform/draft`、`PATCH /transform/{id}`、`POST /transform/{id}/apply`、`POST /transform/{id}/discard` | `DATA_SYNC` |
| `GET /history` | `DATA_READ`（只看本人；`DATA_ADMIN` 看全租户） |

`AnalyticsQueryPort`（供 Step 07 `SQL_TRANSFORM` 算子、Step 09 查询即应用、Step 11 看板组件调用）：`run_sql(principal, sql, params)`、`run_saved(principal, id, params)`、`run_metric(principal, name, query)`、`run_pivot(principal, spec)`。

---

## 04.10 前端

- `AnalyticsStudioPage`（`/analytics`）：左侧 `SchemaTree`（数据集 → 列，标注 PII 与业务术语；平台视图；直连）；中部页签“SQL”（Monaco，DuckDB 关键字高亮、列名补全、`Ctrl+Enter` 运行）、“透视”（`PivotBuilder` 拖拽行 / 列 / 值 / 筛选，实时预览）、“指标”（`MetricManager`）、“AI 处理”（`TransformWizard`：指令 → 预览 → 确认）；底部 `ResultGrid`（虚拟滚动、截断提示）与 `ChartPanel`（ECharts，自动推荐类型可切换）；“保存为查询”。
- `AxQueryPage`：每条回答显示意图标识；`DATA` / `HYBRID` 回答展开 SQL 与结果表、图表；引用段落可点击跳转；点赞 / 点踩 / 修正 SQL。

Playwright：`e2e/analytics.spec.ts` 在 SQL 页签执行 `SELECT count(*) FROM demo_orders` 显示 `100`；透视页签拖入 `status` 到行、`amount` 到值后表格出现“总计”行。

---

## 04.11 基准

`scripts/bench/analytics-data.sh` 生成 1 000 万行 `bench_orders`（10 个维度列、5 个数值列，DuckDB `range()` 生成后写入湖）；`scripts/bench/analytics-bench.sh` 在基准环境（4 vCPU / 8 GB / NVMe SSD）各执行 20 次并输出 P50 / P95：

| 场景 | 断言 |
| :--- | :--- |
| 单维度分组聚合 `SUM` | P95 ≤ 1.5 秒 |
| 两行维度 + 一列维度透视含小计 | P95 ≤ 3 秒 |
| 指标同比查询（12 个月 × 10 个区域） | P95 ≤ 2 秒 |
| 守卫 + 改写（不含执行） | P95 ≤ 5 毫秒 |

基准结果写入 `scripts/bench/results/analytics-<git sha>.json`，不在 CI 中执行。

---

## 04.12 验收

`scripts/seed/04-analytics.sh`：创建指标 `paid_amount`（`SUM(amount)`，过滤 `status='PAID'`，维度 `status`，时间列 `updated_at`）并设为认证；保存查询 `orders_by_status`；输出 `PAID_METRIC_NAME`。

`scripts/gate/step04.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-analytics guard pivot semantic nl2sql_eval)

# 基本查询与租户改写
expect_json "sql-count" "$(api POST /api/analytics/sql/run '{"sql":"SELECT count(*) AS n FROM demo_orders"}')" '.rows[0].n == 100'
expect_json "platform-view" "$(api POST /api/analytics/sql/run '{"sql":"SELECT count(*) AS n FROM ax.nodes WHERE type_name = '\''Warehouse'\''"}')" '.rows[0].n >= 2'

# 守卫拒绝
for q in "SELECT * FROM read_csv('/etc/passwd')" "SELECT getenv('HOME')" "ATTACH ':memory:' AS x" \
         "SELECT * FROM lake.\"tenant_other\".orders" "SELECT * FROM ax_pg.public.sec_users" "COPY demo_orders TO '/tmp/x.csv'"; do
  expect_json "guard:$q" "$(api POST /api/analytics/sql/run "$(jq -nc --arg s "$q" '{sql:$s}')")" '.error_code == "SQL_REJECTED"'
done

# 截断
expect_json "truncate" "$(api POST /api/analytics/sql/run '{"sql":"SELECT * FROM range(20)","max_rows":10}')" '.truncated == true and (.rows | length) == 10'

# 透视小计与总计
expect_json "pivot-total" "$(api POST /api/analytics/pivot '{"dataset":"demo_orders","rows":[{"field":"status"}],"columns":[],"values":[{"field":"amount","agg":"SUM","alias":"amt"}],"filters":[],"sort":[],"subtotals":true,"grand_total":true,"row_limit":100,"column_limit":50}')" '(.row_kinds | index("GRAND_TOTAL")) != null'

# 指标与数据库对账
EXPECTED=$(sql_owner "SELECT round(sum(amount)::numeric, 2) FROM demo_src.orders WHERE status='PAID'")
ACTUAL=$(api POST "/api/analytics/metrics/$PAID_METRIC_NAME/query" '{"dimensions":[],"filters":[],"grain":"MONTH"}' | jq -r '[.rows[].value | tonumber] | add | . * 100 | round / 100')
expect_eq "metric-reconcile" "$ACTUAL" "$EXPECTED"

# PII 脱敏（演示表 phone 列）
expect_json "pii-mask" "$(api POST /api/analytics/sql/run '{"sql":"SELECT phone FROM demo_orders LIMIT 1"}')" '.rows[0].phone | test("^1[3-9][0-9]\\*{4}[0-9]{4}$")'

# 问数（门禁环境使用录制响应：AXPOINT_LLM_CASSETTE 指向评测录制文件）
expect_json "ask-data" "$(api POST /api/analytics/ask '{"question":"已支付订单的总金额是多少"}')" '.intent == "DATA" and (.sql | test("SUM"; "i"))'

# 历史与审计
expect_eq "audit-query" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='QUERY_SQL'")" "t"

gate_pass step04
```

门禁环境中 `AXPOINT_LLM_CASSETTE=axpoint-core/crates/axpoint-analytics/tests/nl2sql_eval/cassette.jsonl` 使 `axpoint-llm` 按提示哈希回放录制响应（未命中时返回 502 `UPSTREAM_LLM`），保证门禁结果确定。

**通过标准**：最后一行 `GATE step04 PASS`；基准环境中 §04.11 全部断言成立。
