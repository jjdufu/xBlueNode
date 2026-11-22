# 04 · Step 03：AxGraph 存储扩展、时序与图查询

AxGraph 由两层组成：**超图层**（`hg_docs / hg_paras / hg_entities / hg_edges / hg_embeddings`，知识与文档）与**业务图层**（`xb_nodes / xb_links`，业务对象与关系）。本步把业务图层扩展为可承载生产数据的存储：节点静态属性与地理坐标、时序指标与多粒度汇总（含去重计数草图与频次分布）、对象向量、实体归一化与“实体 ↔ 节点”链接、图遍历与图算法。

---

## 03.0 目标、前置与契约

- **目标**：节点与关系可批量写入且幂等；指标写入时序表并自动汇总到分钟 / 小时 / 日 / 月；按时间窗口聚合查询；节点支持全文 + 向量混合检索与地理范围检索；超图实体与业务节点可自动关联；k 跳邻居、最短路径、PageRank、度中心性、标签传播社区、连通分量可在 API 中调用。
- **前置**：Step 00 通过。
- **契约清单**：`0004_axgraph_timeseries.sql`；`axpoint-domain::graph`（`NodeRecord`、`LinkRecord`、`MetricPoint`、`RollupRow`、`WindowQuery`、`GraphAlgo`、`EntityLink`）；端口 `GraphStorePort`、`MetricStorePort`、`GraphQueryPort`；`docs/API_CONTRACT.md` 的 `/api/xblue/nodes*`、`/api/xblue/links*`、`/api/xblue/metrics*`、`/api/xblue/graph/*`、`/api/xblue/entities/*`；`lib/api/xblue.ts` 类型。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0004_axgraph_timeseries.sql` | §03.1 |
| 新建 | `axpoint-core/crates/axpoint-domain/src/graph.rs` | 类型与纯算法（汇总合并、HLL 合并、频次截断） |
| 修改 | `axpoint-core/crates/axpoint-domain/src/ports.rs` | 三个端口 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/topology.rs` | `BlueNode` 增加 `properties`、`geo`、`updated_at`；`UpsertBlueNodeRequest` 增加 `properties`、`geo`、`metrics_merge` |
| 新建 | `axpoint-core/crates/axpoint-db/src/{nodes.rs, metrics.rs, rollup.rs, graph_query.rs, entity_link.rs}` | 端口实现 |
| 修改 | `axpoint-core/crates/axpoint-db/src/topology.rs` | `upsert_blue_node` 改为指标合并与关系幂等写入 |
| 新建 | `axpoint-core/crates/axpoint-domain/src/algo/{mod.rs, pagerank.rs, label_propagation.rs, components.rs, paths.rs}` | 基于 `petgraph` 的图算法 |
| 新建 | `axpoint-core/crates/axpoint-server/src/workers/{rollup_worker.rs, embed_worker.rs, partition_worker.rs}` | `ROLLUP`、`EMBED_NODES` 任务，分区维护 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/xblue.rs` | §03.7 路由 |
| 修改 | `axpoint-core/crates/axpoint-llm/src/lib.rs` | `AiGateway::embed_batch` 增加按 64 条分块调用与 429 退避重试 |
| 修改 | `xbluenode-web/src/pages/XBlueGraphPage.tsx`、新建 `src/components/xbluegraph/{NodeInspector.tsx, MetricChart.tsx, NeighborExpand.tsx, PathFinder.tsx, AlgoPanel.tsx}` | 图浏览增强 |
| 新建 | `scripts/seed/03-axgraph.sh` | 演示节点、关系、30 日每小时指标 |
| 新建 | `scripts/gate/step03.sh` | 门禁 |

---

## 03.1 迁移 `deploy/migrations/0004_axgraph_timeseries.sql`

```sql
-- 0004_axgraph_timeseries.sql：节点属性与地理、关系幂等、时序指标与汇总、实体归一化与链接、图算法结果

-- 业务节点扩展
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS properties        JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS geo_lng           DOUBLE PRECISION CHECK (geo_lng BETWEEN -180 AND 180);
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS geo_lat           DOUBLE PRECISION CHECK (geo_lat BETWEEN -90 AND 90);
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS source_dataset_id UUID;
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS source_row_key    TEXT;
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW();
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS row_version       BIGINT NOT NULL DEFAULT 1;
ALTER TABLE xb_nodes ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(title,'') || ' ' || coalesce(node_key,'') || ' ' || coalesce(type_name,''))) STORED;
CREATE INDEX IF NOT EXISTS idx_xb_nodes_type  ON xb_nodes (tenant_id, type_name);
CREATE INDEX IF NOT EXISTS idx_xb_nodes_fts   ON xb_nodes USING GIN (fts);
CREATE INDEX IF NOT EXISTS idx_xb_nodes_geo   ON xb_nodes (tenant_id, geo_lng, geo_lat) WHERE geo_lng IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_xb_nodes_props ON xb_nodes USING GIN (properties jsonb_path_ops);

-- 关系扩展与幂等：同租户、同类型、同端点只保留一条
ALTER TABLE xb_links ADD COLUMN IF NOT EXISTS properties        JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE xb_links ADD COLUMN IF NOT EXISTS source_dataset_id UUID;
ALTER TABLE xb_links ADD COLUMN IF NOT EXISTS valid_from        TIMESTAMPTZ;
ALTER TABLE xb_links ADD COLUMN IF NOT EXISTS valid_to          TIMESTAMPTZ;
ALTER TABLE xb_links ADD COLUMN IF NOT EXISTS updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW();
DELETE FROM xb_links a USING xb_links b
 WHERE a.tenant_id = b.tenant_id AND a.link_type = b.link_type
   AND a.src_node_id = b.src_node_id AND a.dst_node_id = b.dst_node_id AND a.created_at > b.created_at;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_links_endpoints ON xb_links (tenant_id, link_type, src_node_id, dst_node_id);
CREATE INDEX IF NOT EXISTS idx_xb_links_src ON xb_links (tenant_id, src_node_id);
CREATE INDEX IF NOT EXISTS idx_xb_links_dst ON xb_links (tenant_id, dst_node_id);

-- 时序原始点（按月分区）
CREATE TABLE IF NOT EXISTS xb_metric_series (
    tenant_id  TEXT NOT NULL,
    node_id    UUID NOT NULL,
    metric     TEXT NOT NULL CHECK (metric ~ '^[a-z][a-z0-9_]{0,62}$'),
    ts         TIMESTAMPTZ NOT NULL,
    value      DOUBLE PRECISION,
    value_text TEXT,                                -- 分类型指标（状态、等级）
    source_tag TEXT NOT NULL CHECK (source_tag IN ('SYNC','AGGREGATION','DERIVED','ACTION','MANUAL','ALGORITHM')),
    batch_id   UUID,
    PRIMARY KEY (tenant_id, node_id, metric, ts, source_tag),
    CHECK (value IS NOT NULL OR value_text IS NOT NULL)
) PARTITION BY RANGE (ts);
CREATE INDEX IF NOT EXISTS idx_xb_metric_series_metric_ts ON xb_metric_series (tenant_id, metric, ts);

-- 默认分区兜底，避免越界写入失败；分区维护任务把落入默认分区的数据迁出
CREATE TABLE IF NOT EXISTS xb_metric_series_default PARTITION OF xb_metric_series DEFAULT;
SELECT ax_enable_tenant_rls('xb_metric_series');
SELECT ax_enable_tenant_rls('xb_metric_series_default');

CREATE OR REPLACE FUNCTION ax_ensure_metric_partitions(p_months_back INT, p_months_ahead INT) RETURNS INT
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public AS $$
DECLARE
    m DATE; created INT := 0; part TEXT;
BEGIN
    FOR m IN SELECT generate_series(date_trunc('month', now()) - make_interval(months => p_months_back),
                                    date_trunc('month', now()) + make_interval(months => p_months_ahead),
                                    interval '1 month')::date LOOP
        part := format('xb_metric_series_%s', to_char(m, 'YYYYMM'));
        IF to_regclass(part) IS NULL THEN
            EXECUTE format('CREATE TABLE %I PARTITION OF xb_metric_series FOR VALUES FROM (%L) TO (%L)',
                           part, m, (m + interval '1 month')::date);
            PERFORM ax_enable_tenant_rls(part::regclass);
            created := created + 1;
        END IF;
    END LOOP;
    RETURN created;
END $$;
SELECT ax_ensure_metric_partitions(24, 3);
REVOKE ALL ON FUNCTION ax_ensure_metric_partitions(INT, INT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_ensure_metric_partitions(INT, INT) TO axpoint_app;

-- 跨租户安全回收历史月分区：仅当分区结束日早于全体活跃租户的最大保留期时整表 DROP
CREATE OR REPLACE FUNCTION ax_drop_expired_metric_partitions() RETURNS INT
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public AS $$
DECLARE
    max_keep INT; cutoff DATE; r RECORD; dropped INT := 0; part_end DATE;
BEGIN
    SELECT COALESCE(MAX(rp.keep_days), 730) INTO max_keep
      FROM ax_retention_policies rp
      JOIN sec_tenants st ON st.tenant_id = rp.tenant_id
     WHERE rp.target = 'METRIC_SERIES' AND st.status = 'ACTIVE';
    cutoff := date_trunc('month', now() - make_interval(days => max_keep))::date;
    FOR r IN SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
              WHERE i.inhparent = 'xb_metric_series'::regclass
                AND c.relname ~ '^xb_metric_series_[0-9]{6}$' LOOP
        part_end := (to_date(substring(r.relname FROM 18 FOR 6), 'YYYYMM') + interval '1 month')::date;
        IF part_end <= cutoff THEN
            EXECUTE format('DROP TABLE IF EXISTS %I', r.relname);
            dropped := dropped + 1;
        END IF;
    END LOOP;
    RETURN dropped;
END $$;
REVOKE ALL ON FUNCTION ax_drop_expired_metric_partitions() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_drop_expired_metric_partitions() TO axpoint_app;

-- 多粒度汇总
CREATE TABLE IF NOT EXISTS xb_metric_rollups (
    tenant_id    TEXT NOT NULL,
    node_id      UUID NOT NULL,
    metric       TEXT NOT NULL,
    grain        TEXT NOT NULL CHECK (grain IN ('MINUTE','HOUR','DAY','MONTH')),
    bucket_start TIMESTAMPTZ NOT NULL,
    cnt          BIGINT NOT NULL,
    sum_v        DOUBLE PRECISION,
    min_v        DOUBLE PRECISION,
    max_v        DOUBLE PRECISION,
    last_v       DOUBLE PRECISION,
    last_text    TEXT,
    last_ts      TIMESTAMPTZ NOT NULL,
    hll          BYTEA,                 -- value_text 的 HyperLogLog++ 草图（精度 14），用于 COUNT_DISTINCT
    freq         JSONB,                 -- value_text 频次分布，保留 Top 100，其余合并为 "__other__"
    PRIMARY KEY (tenant_id, node_id, metric, grain, bucket_start)
);
CREATE INDEX IF NOT EXISTS idx_xb_rollups_metric ON xb_metric_rollups (tenant_id, metric, grain, bucket_start);

-- 汇总水位：每租户每粒度已完成的最后一个桶
CREATE TABLE IF NOT EXISTS xb_rollup_watermarks (
    tenant_id   TEXT NOT NULL,
    grain       TEXT NOT NULL CHECK (grain IN ('MINUTE','HOUR','DAY','MONTH')),
    done_until  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, grain)
);

-- 迟到数据标记：写入时间早于水位的点所在桶需要重算
CREATE TABLE IF NOT EXISTS xb_rollup_dirty (
    tenant_id    TEXT NOT NULL,
    node_id      UUID NOT NULL,
    metric       TEXT NOT NULL,
    minute_start TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, node_id, metric, minute_start)
);

-- 实体归一化与链接
ALTER TABLE hg_entities ADD COLUMN IF NOT EXISTS name_norm TEXT;
UPDATE hg_entities SET name_norm = lower(btrim(name)) WHERE name_norm IS NULL;
CREATE INDEX IF NOT EXISTS idx_hg_entities_norm ON hg_entities (tenant_id, name_norm);

CREATE TABLE IF NOT EXISTS xb_entity_links (
    tenant_id    TEXT NOT NULL,
    entity_id    UUID NOT NULL REFERENCES hg_entities(id) ON DELETE CASCADE,
    node_id      UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    match_method TEXT NOT NULL CHECK (match_method IN ('EXACT_KEY','EXACT_TITLE','SYNONYM','EMBEDDING','MANUAL')),
    confidence   DOUBLE PRECISION NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    confirmed    BOOLEAN NOT NULL DEFAULT FALSE,
    rejected     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_id, node_id)
);
CREATE INDEX IF NOT EXISTS idx_xb_entity_links_node ON xb_entity_links (tenant_id, node_id);

-- 图算法结果
CREATE TABLE IF NOT EXISTS xb_graph_algo_results (
    tenant_id   TEXT NOT NULL,
    algo        TEXT NOT NULL CHECK (algo IN ('PAGERANK','DEGREE','LABEL_PROPAGATION','CONNECTED_COMPONENTS')),
    params_hash TEXT NOT NULL,
    node_id     UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    score       DOUBLE PRECISION,
    community   BIGINT,
    computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, algo, params_hash, node_id)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xb_metric_rollups','xb_rollup_watermarks','xb_rollup_dirty','xb_entity_links','xb_graph_algo_results'
]::regclass[]) AS t;

-- 跨租户汇总调度
CREATE OR REPLACE FUNCTION ax_rollup_tenants() RETURNS SETOF TEXT
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT tenant_id FROM sec_tenants WHERE status = 'ACTIVE';
$$;
REVOKE ALL ON FUNCTION ax_rollup_tenants() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_rollup_tenants() TO axpoint_app;
```

---

## 03.2 节点与关系写入

### 03.2.1 语义

| 操作 | 语义 |
| :--- | :--- |
| 节点 upsert | 键 `(tenant_id, type_name, node_key)`；`title`、`properties`、`geo` 覆盖写入；`metrics`（最新值快照）按 `metrics_merge`：`MERGE`（默认，`metrics = metrics || $new`）或 `REPLACE`；`row_version + 1` |
| 乐观并发 | 请求带 `if_row_version` 时，`UPDATE ... WHERE row_version = $v`，不匹配返回 409 `VERSION_CONFLICT` |
| 关系 upsert | `INSERT ... ON CONFLICT (tenant_id, link_type, src_node_id, dst_node_id) DO UPDATE SET weight, properties, valid_from, valid_to, updated_at` |
| 批量写入 | `POST /api/xblue/nodes/bulk`：最多 5 000 个节点 + 20 000 条关系；节点用 `UNNEST($1::text[], $2::text[], ...)` 一条语句写入；关系端点可用 `{type_name, node_key}` 引用，先批量解析为 ID，解析不到的关系计入 `unresolved` 返回 |
| 删除 | 节点删除级联删除关系（外键已有 `ON DELETE CASCADE`）与 `xb_metric_series` 中该节点数据（同一事务 `DELETE`） |
| 事件 | 节点写入产生 `ax_outbox('node.upserted', {node_id, type_name})`；`metrics` 变化产生 `node.metrics_changed`（批量写入时每批合并为一条事件，`payload.node_ids` 数组，最多 1 000 个） |

### 03.2.2 `UpsertBlueNodeRequest`

```rust
/// 节点写入请求。租户来自 AuthPrincipal。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertBlueNodeRequest {
    pub type_name: String,
    pub node_key: String,
    pub title: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub metrics: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub metrics_merge: MetricsMerge,          // MERGE | REPLACE
    #[serde(default)]
    pub geo: Option<GeoPoint>,                // { lng, lat }
    #[serde(default)]
    pub if_row_version: Option<i64>,
    #[serde(default)]
    pub link_to_node_id: Option<Uuid>,
    #[serde(default)]
    pub link_type: Option<String>,
    #[serde(default)]
    pub link_weight: Option<f64>,
}
```

`link_to_node_id` 存在而 `link_type` 缺失时返回 400 `INVALID_ARGUMENT`（不再默认任何关系类型）；`link_weight` 缺失时取 `xb_edge_types.impact_weight`，关系类型未在模型中定义时返回 400 `SCHEMA_INVALID`。

---

## 03.3 时序指标与多粒度汇总

### 03.3.1 写入

`POST /api/xblue/metrics/write`（`DATA_SYNC`）：

```json
{
  "points": [
    {"node": {"type_name": "Warehouse", "node_key": "W-1"}, "metric": "inventory", "ts": "2026-01-31T08:00:00Z", "value": 480},
    {"node_id": "5f0c...", "metric": "status", "ts": "2026-01-31T08:00:00Z", "value_text": "NORMAL"}
  ],
  "source_tag": "SYNC"
}
```

- 单次最多 50 000 点；`INSERT ... ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET value, value_text`。
- 同一事务内：每个 `(node, metric)` 的最大 `ts` 点写入 `xb_nodes.metrics`（只在新点 `ts` ≥ 快照时间时覆盖，快照时间存于 `properties.__metric_ts.<metric>`）；早于 `MINUTE` 水位的点写入 `xb_rollup_dirty`。
- 提交后入队 `ROLLUP`（幂等键 `<tenant>:<当前分钟>`）。

### 03.3.2 汇总任务（`workers/rollup_worker.rs`）

1. `MINUTE`：从 `done_until` 到 `date_trunc('minute', now()) - 1 minute` 的原始点按分钟聚合写入（`ON CONFLICT DO UPDATE`），再重算 `xb_rollup_dirty` 中的分钟并删除脏标记。
2. `HOUR` / `DAY` / `MONTH`：由下一级汇总合并（`cnt` 相加、`sum` 相加、`min/max` 取极值、`last` 取 `last_ts` 最大者、`hll` 合并、`freq` 合并后截断 Top 100），桶完成后推进水位；受脏分钟影响的上级桶同时重算。
3. 桶边界按租户时区计算 `DAY` / `MONTH`（`date_trunc('day', ts AT TIME ZONE tz) AT TIME ZONE tz`），`MINUTE` / `HOUR` 按 UTC。
4. HLL：`hyperloglogplus::HyperLogLogPlus<String, RandomState>`，序列化用 `serde_json` 后存 `BYTEA`；合并函数 `merge_hll(a, b)` 在 `axpoint-domain::graph` 中实现并有单测（10 万不同值误差 < 2%）。
5. 汇总表保留策略：`METRIC_ROLLUP_MINUTE`（默认 `keep_days=30`）只作用于 `MINUTE` 粒度；原始点按 `METRIC_SERIES` 清理：先在租户事务内删除早于该租户 `keep_days` 的过期点，再调用 `ax_drop_expired_metric_partitions()` 对早于全体活跃租户最大保留期的历史月分区整表 `DROP TABLE`。

### 03.3.3 查询

| 接口 | 说明 |
| :--- | :--- |
| `GET /api/xblue/nodes/{id}/metrics/{metric}?from=&to=&grain=&agg=` | `grain` 缺省时按区间自动选择：≤ 6 小时 `RAW`，≤ 7×24 小时 `MINUTE`，≤ 90×24 小时 `HOUR`，其余 `DAY`；`agg ∈ AggFn`；返回 `[{ts, value}]`，最多 5 000 点（超过时自动升一级粒度） |
| `POST /api/xblue/metrics/window` | 窗口聚合：`{node_ids? , type_name?, metric, from, to, window: {kind: "TUMBLING"|"SLIDING", size: "1h", step?: "15m"}, agg, group_by?: "node"|"type"|"none"}`；`TUMBLING` 直接用对应粒度汇总表，`SLIDING` 在分钟汇总上用 `SUM(...) OVER (ORDER BY bucket_start RANGE BETWEEN interval PRECEDING AND CURRENT ROW)` 计算 |
| `GET /api/xblue/metrics/catalog` | 当前租户全部 `(type_name, metric, 点数, 最早 ts, 最近 ts)`，供 Step 04 语义层与 Step 05 建模使用 |

`COUNT_DISTINCT` 只对分类型指标有效，返回 HLL 估计值；`MEDIAN` 在 `RAW` 粒度精确计算，在汇总粒度返回 400 `INVALID_ARGUMENT`（提示改用 `RAW`）。

---

## 03.4 对象向量与混合检索

- 节点文本：`"{type display_name}：{title}（{node_key}）" + properties 中 STRING 类型字段的 "键: 值" 拼接`，截断 2 000 字符。
- `EMBED_NODES` 任务：节点 upsert 后入队（幂等键 `<tenant>:<node_id>:<row_version>`），调用 `AiGateway::embed_batch`，写 `hg_embeddings(node_type='XB_NODE', node_id, model_name, embedding_dim, embedding)`（`ON CONFLICT (node_type, node_id, model_name) DO UPDATE`）。
- `GET /api/xblue/nodes/search?q=&type_name=&k=20`：FTS（`ts_rank_cd(fts, plainto_tsquery('simple', q))`）与向量（`embedding <=> $q` 走对应维度的 HNSW 部分索引）各取 50 条，按 RRF（`k=60`）融合后返回前 `k` 条，每条带 `channels: ["FTS","VECTOR"]`。
- `GET /api/xblue/nodes?bbox=minLng,minLat,maxLng,maxLat&type_name=`：`geo_lng BETWEEN ... AND geo_lat BETWEEN ...`，最多 10 000 条，供 Step 11 地图组件使用。

---

## 03.5 实体归一化与“实体 ↔ 节点”链接

### 03.5.1 归一化函数（`axpoint-domain::graph::normalize_name`）

1. Unicode NFKC（全角转半角）。
2. 去除首尾空白，内部连续空白压缩为一个空格。
3. 转小写。
4. 去除末尾常见组织后缀：`有限公司`、`股份有限公司`、`集团`、`co., ltd.`、`inc.`、`ltd`（后缀表 `normalize_suffixes.txt`，随代码提交）。

文档入库时（现有 `ingest_document`）写 `hg_entities.name_norm = normalize_name(name)`，同义词数组同样归一化后比对。

### 03.5.2 链接算法（`POST /api/xblue/entities/resolve`，也由 `node.upserted` 与文档入库后自动触发，按租户增量执行）

| 顺序 | 方法 | 判定 | 置信度 |
| :--- | :--- | :--- | :--- |
| 1 | `EXACT_KEY` | `normalize(entity.name) = normalize(node.node_key)` | 1.0 |
| 2 | `EXACT_TITLE` | `normalize(entity.name) = normalize(node.title)` | 0.95 |
| 3 | `SYNONYM` | 实体任一同义词等于节点键或标题（归一化后） | 0.9 |
| 4 | `EMBEDDING` | 实体名向量与节点向量余弦相似度 ≥ 0.88，且同一实体的第二候选与第一候选相差 ≥ 0.03 | 相似度值 |

- 置信度 ≥ 0.9 的链接自动 `confirmed=true`；其余进入待确认列表 `GET /api/xblue/entities/candidates`，人工 `POST /api/xblue/entities/links/{entity_id}/{node_id}/confirm|reject`。`rejected=true` 的组合不会再被自动生成。
- 链接用途：AxQuery 召回命中实体时附带关联节点与最新指标（Step 04 混合问答）；`GET /api/xblue/nodes/{id}/documents` 返回提及该节点的文档与段落（经 `MENTIONS` 边）；Step 06 的知识 ↔ 节点同步。

---

## 03.6 图查询与图算法

### 03.6.1 遍历（`axpoint-db/src/graph_query.rs`）

| 接口 | 实现 | 限制 |
| :--- | :--- | :--- |
| `GET /api/xblue/graph/neighbors?node_id=&depth=1..4&direction=OUT|IN|BOTH&link_types=A,B&limit=500` | 递归 CTE，`path` 数组防环，`depth` 逐层截断 | 返回节点 ≤ `limit`（≤ 2 000），超出时 `truncated=true` |
| `GET /api/xblue/graph/path?from=&to=&max_depth=6&weighted=false` | 双向 BFS：每侧递归 CTE 展开到 `max_depth/2`，取交集重建路径；`weighted=true` 时把两端 `max_depth` 范围内的子图读入内存用 `petgraph::algo::dijkstra`（权重 `1/weight`） | 子图边数 ≤ 200 000，否则 400 `LIMIT_OUT_OF_RANGE` |
| `GET /api/xblue/graph/subgraph?type_names=&link_types=&limit=2000` | 按类型取节点及其之间的关系 | 供图浏览器初始画布 |

### 03.6.2 算法（`POST /api/xblue/graph/algorithms/{algo}`，`STUDIO_EDIT`）

请求 `{type_names?: [], link_types?: [], params: {...}}`；把符合条件的节点与关系读入 `petgraph::Graph`（边数 ≤ 500 000），计算后写 `xb_graph_algo_results`（同一 `params_hash` 覆盖），并把分数写入 `xb_metric_series(source_tag='ALGORITHM', metric='algo_<algo>')` 便于在看板展示与在规则中引用。

| `algo` | 参数 | 实现 |
| :--- | :--- | :--- |
| `PAGERANK` | `damping=0.85, max_iter=100, tol=1e-6` | 幂迭代，悬挂节点均分 |
| `DEGREE` | `direction` | 入度 / 出度 / 总度，归一化到 `[0,1]` |
| `LABEL_PROPAGATION` | `max_iter=50, seed=42` | 异步标签传播，节点顺序由种子打乱，结果确定 |
| `CONNECTED_COMPONENTS` | — | `petgraph::algo::kosaraju_scc`（有向强连通）或并查集（无向弱连通，`params.weak=true`） |

响应：`{algo, params_hash, node_count, edge_count, duration_ms, top: [{node_id, title, score, community}]}`（前 50）。`GET /api/xblue/graph/algorithms/{algo}/results?params_hash=` 分页读取全量结果。

**测试**（`axpoint-domain/tests/algo.rs`）：5 节点环 + 1 悬挂节点的 PageRank 与 networkx 参考值（写入测试常量）误差 < 1e-6；两个不相连三角形的标签传播得到 2 个社区；SCC 用例覆盖自环与孤立点。

---

## 03.7 路由汇总（`routes/xblue.rs`）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /nodes`（`?type_name=&q=&bbox=&limit=&cursor=`）、`POST /nodes`、`POST /nodes/bulk` | `DATA_READ` / `STUDIO_EDIT` |
| `GET /nodes/{id}`、`PATCH /nodes/{id}`、`DELETE /nodes/{id}` | `DATA_READ` / `STUDIO_EDIT` |
| `GET /nodes/search` | `DATA_READ` |
| `GET /nodes/{id}/documents` | `DATA_READ` |
| `GET /nodes/{id}/metrics/{metric}` | `DATA_READ` |
| `GET /links`（`?node_id=&link_type=`）、`POST /links`、`DELETE /links/{id}` | `DATA_READ` / `STUDIO_EDIT` |
| `POST /metrics/write` | `DATA_SYNC` |
| `POST /metrics/window`、`GET /metrics/catalog` | `DATA_READ` |
| `GET /graph/neighbors`、`GET /graph/path`、`GET /graph/subgraph` | `DATA_READ` |
| `POST /graph/algorithms/{algo}`、`GET /graph/algorithms/{algo}/results` | `STUDIO_EDIT` / `DATA_READ` |
| `POST /entities/resolve`、`GET /entities/candidates`、`POST /entities/links/{entity_id}/{node_id}/confirm`、`POST /entities/links/{entity_id}/{node_id}/reject` | `STUDIO_EDIT` |

以上路径均挂在 `/api/xblue` 下；`GET|POST /api/xblue/schema` 与 `GET|POST /api/xblue/nodes` 保留现有行为并按本步语义扩展。

---

## 03.8 前端 `XBlueGraphPage`

- 画布：ECharts `graph`（力导向，节点 ≤ 2 000）；节点颜色按类型，大小按所选指标或 PageRank 分数。
- `NodeInspector`：属性、最新指标、关联文档（来自 `/nodes/{id}/documents`）、实体链接、`row_version`。
- `MetricChart`：指标趋势（自动粒度）、窗口聚合切换。
- `NeighborExpand`：双击节点展开 1 跳；右键“展开 2 跳 / 只看某关系类型”。
- `PathFinder`：选两个节点求最短路径并高亮。
- `AlgoPanel`：运行四种算法并按结果着色 / 定大小。

Playwright：`e2e/xbluegraph.spec.ts` 打开 `/graph`，双击 `华东仓` 后画布节点数增加，打开 `inventory` 趋势图出现折线。

---

## 03.9 验收

`scripts/seed/03-axgraph.sh`：建模型 `Factory → Warehouse → Order`（`SUPPLIES`、`FULFILLS`），写 3 个工厂（带经纬度，F-1 位于 `121.47,31.23`）、2 个仓库（W-1 复用 Step 00 种子，W-2 新建）、20 个订单（键 `O-3`～`O-22`）与关系（F-1 供应全部仓库，每个订单恰好由一个仓库履约）；为每个仓库写 30 × 24 个小时级 `inventory` 点（值由 `100 + (小时序号 % 24)` 生成，可精确计算期望汇总）；写一篇提及“华东仓”的文档。输出 `FACTORY1_NODE_ID`、`ORDER20_NODE_ID` 等到 `.gate/ids.env`。

`scripts/gate/step03.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain algo graph && cargo test -p axpoint-db nodes metrics rollup graph_query entity_link)

# 幂等写入：重复写同一关系不产生重复
api POST /api/xblue/links "{\"link_type\":\"FULFILLS\",\"src_node_id\":\"$WAREHOUSE_NODE_ID\",\"dst_node_id\":\"$ORDER1_NODE_ID\",\"weight\":0.8}" >/dev/null
expect_eq "link-unique" "$(sql_app default "SELECT count(*) FROM xb_links WHERE src_node_id='$WAREHOUSE_NODE_ID' AND dst_node_id='$ORDER1_NODE_ID' AND link_type='FULFILLS'")" "1"

# 指标合并而非覆盖
api POST /api/xblue/nodes '{"type_name":"Warehouse","node_key":"W-1","title":"华东仓","metrics":{"capacity":900}}' >/dev/null
expect_json "metrics-merge" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID)" '.metrics.capacity == 900 and (.metrics | has("inventory"))'

# 汇总：日粒度 SUM 与种子数据的精确值一致（每日 24 点，值 100..123，和 = 2676）
wait_job "$(api POST /api/axops/jobs/rollup-now | jq -r .job_id)"
expect_json "rollup-day" "$(api GET "/api/xblue/nodes/$WAREHOUSE_NODE_ID/metrics/inventory?from=$SEED_DAY1&to=$SEED_DAY2&grain=DAY&agg=SUM")" '.[0].value == 2676'
expect_json "window-sliding" "$(api POST /api/xblue/metrics/window "{\"node_ids\":[\"$WAREHOUSE_NODE_ID\"],\"metric\":\"inventory\",\"from\":\"$SEED_DAY1\",\"to\":\"$SEED_DAY2\",\"window\":{\"kind\":\"SLIDING\",\"size\":\"3h\",\"step\":\"1h\"},\"agg\":\"AVG\"}")" '(.items | length) == 24'

# 检索与地理
expect_json "node-search" "$(api GET '/api/xblue/nodes/search?q=华东')" '.items[0].node_key == "W-1"'
expect_json "geo-bbox"    "$(api GET '/api/xblue/nodes?bbox=120,30,122,32&type_name=Factory')" '(.items | length) >= 1'

# 实体链接
api POST /api/xblue/entities/resolve '{}' >/dev/null
expect_json "entity-link" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID/documents)" '(.items | length) >= 1'

# 图查询与算法
expect_json "neighbors" "$(api GET "/api/xblue/graph/neighbors?node_id=$WAREHOUSE_NODE_ID&depth=1&direction=BOTH")" '(.nodes | length) >= 3'
expect_json "path"      "$(api GET "/api/xblue/graph/path?from=$FACTORY1_NODE_ID&to=$ORDER20_NODE_ID&max_depth=4")" '(.path | length) == 3'
expect_json "pagerank"  "$(api POST /api/xblue/graph/algorithms/PAGERANK '{"params":{}}')" '.node_count >= 25 and (.top | length) > 0'

# 行级安全覆盖分区子表
expect_eq "rls-partition" "$(sql_app other "SELECT count(*) FROM xb_metric_series")" "0"

gate_pass step03
```

`POST /api/axops/jobs/rollup-now`（`OPS_ADMIN`，Step 03 在 `routes/axops.rs` 中加入）为当前租户立即入队一次 `ROLLUP` 任务并返回 `job_id`；`SEED_DAY1`、`SEED_DAY2` 由种子脚本写入 `.gate/ids.env`（种子数据首日的 00:00 与次日 00:00，租户时区）。

**通过标准**：最后一行 `GATE step03 PASS`。
