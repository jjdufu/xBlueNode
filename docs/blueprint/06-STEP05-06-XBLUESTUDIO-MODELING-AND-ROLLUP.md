# 06 · Step 05：xBlueStudio 建模、AI 草案与模型版本 ／ Step 06：表达式引擎、映射物化、派生与动态聚合、知识同步

本文件包含两个相邻 Step。Step 05 交付“模型如何被定义、审阅与发布”；Step 06 交付“已发布的模型如何驱动数据”：把数据集映射为节点与关系，按派生规则计算指标，按聚合规则沿关系逐级汇总，并把业务对象同步为可检索的知识。

---

# Step 05：xBlueStudio 建模、多子图协同、AI 草案与领域对象接口

## 05.0 目标、前置与契约

- **目标**：可视化画布编辑节点类型、关系类型（含基数）、属性、指标、聚合规则、派生规则与数据映射；支持部门级多子图隔离建模、跨子图实体对齐预览与一键合并入主图；所有编辑先进入草案，校验通过后以版本形式发布（并发安全、可比较、可回滚）；AI 根据数据集、文档或描述生成模型草案；主图模型发布后自动生成并热更新强类型领域对象 REST 接口与动态 GraphQL 接口。
- **前置**：Step 03、Step 04 通过。
- **契约清单**：`0006_xbluestudio_modeling.sql`；`axpoint-domain::studio`（`SchemaDoc`、`SubgraphInfo`、`SubgraphMergeStrategy`、`MergePreview`、`NodeTypeSpec`、`EdgeTypeSpec`、`PropertySpec`、`AggRuleSpec`、`DerivedRuleSpec`、`MappingSpec`、`SchemaDiff`、`ValidationIssue`）；`docs/API_CONTRACT.md` 的 `/api/xblue/schema*`、`/api/xblue/subgraphs*`、`/api/xblue/drafts*`、`/api/xblue/mappings*`、`/api/xblue/graphql*`、`/api/xblue/objects*`；`lib/api/xblue.ts`；A6 Step 05 依赖（`async-graphql`、`async-graphql-axum`）。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0006_xbluestudio_modeling.sql` | §05.1 |
| 新建 | `axpoint-core/crates/axpoint-domain/src/studio/{mod.rs, doc.rs, validate.rs, diff.rs, merge.rs}` | 模型文档、校验、差异、跨子图对齐与合并算法 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/topology.rs` | `PropertyDef`、`AggRuleDef` 扩展为 §05.2 字段（序列化名不变） |
| 新建 | `axpoint-core/crates/axpoint-db/src/studio_repo.rs` | 子图、草案、版本、发布与合并事务 |
| 新建 | `axpoint-core/crates/axpoint-llm/src/schema_draft.rs` | AI 草案提示与结构化输出 |
| 新建 | `axpoint-core/crates/axpoint-analytics/src/profiling.rs` | 外键推断、基数估计（DuckDB 内部查询） |
| 新建 | `axpoint-core/crates/axpoint-server/src/xblue_graphql.rs` | 基于已发布主图 `SchemaDoc` 动态构建并缓存 `async_graphql::dynamic::Schema`（§05.6） |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/xblue.rs` | §05.7 路由（含子图协同、动态 GraphQL 与强类型领域对象 REST） |
| 修改 | `xbluenode-web/src/pages/XBlueStudioPage.tsx`；新建 `src/components/xbluestudio/{StudioCanvas.tsx, StudioSubgraphBar.tsx, SubgraphMergeDialog.tsx, NodeTypeCard.tsx, EdgeTypeEditor.tsx, PropertyTable.tsx, RuleEditor.tsx, MappingEditor.tsx, VersionPanel.tsx, DiffView.tsx, AiDraftWizard.tsx, DomainApiDrawer.tsx, IssueList.tsx}` | 建模工作台、子图切换与合并向导、领域接口调试抽屉 |
| 新建 | `scripts/seed/05-studio.sh` | 演示模型发布、映射定义 |
| 新建 | `scripts/gate/step05.sh` | 门禁 |

---

## 05.1 迁移 `deploy/migrations/0006_xbluestudio_modeling.sql`

```sql
-- 0006_xbluestudio_modeling.sql：多子图空间、类型元数据扩展、派生规则、模型草案、模型版本、数据映射与运行记录

CREATE TABLE IF NOT EXISTS xb_subgraphs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    slug         TEXT NOT NULL CHECK (slug ~ '^[a-z][a-z0-9_-]{0,62}$'),
    display_name TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    owner_dept   TEXT NOT NULL DEFAULT 'default',
    is_main      BOOLEAN NOT NULL DEFAULT FALSE,
    created_by   UUID,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug)
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_subgraphs_main ON xb_subgraphs (tenant_id) WHERE is_main;

ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS subgraph_slug     TEXT NOT NULL DEFAULT 'main';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS description       TEXT NOT NULL DEFAULT '';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS icon              TEXT NOT NULL DEFAULT 'box';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS color             TEXT NOT NULL DEFAULT '#2563eb';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS title_template    TEXT NOT NULL DEFAULT '{node_key}';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS derived_rules     JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS sync_to_knowledge BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW();
ALTER TABLE xb_node_types DROP CONSTRAINT IF EXISTS xb_node_types_tenant_id_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_node_types_subgraph ON xb_node_types (tenant_id, subgraph_slug, name);

ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS subgraph_slug TEXT NOT NULL DEFAULT 'main';
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS cardinality   TEXT NOT NULL DEFAULT 'MANY_TO_MANY'
    CHECK (cardinality IN ('ONE_TO_ONE','ONE_TO_MANY','MANY_TO_ONE','MANY_TO_MANY'));
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS description   TEXT NOT NULL DEFAULT '';
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS properties    JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS propagation   JSONB NOT NULL DEFAULT '{}'::jsonb;  -- AxSim 传导参数，结构见 Step 10
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW();
ALTER TABLE xb_edge_types DROP CONSTRAINT IF EXISTS xb_edge_types_tenant_id_name_src_type_name_dst_type_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_edge_types_subgraph
    ON xb_edge_types (tenant_id, subgraph_slug, name, src_type_name, dst_type_name);

CREATE TABLE IF NOT EXISTS xb_schema_heads (
    tenant_id       TEXT NOT NULL,
    subgraph_slug   TEXT NOT NULL DEFAULT 'main',
    current_version INT NOT NULL DEFAULT 0,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, subgraph_slug)
);

CREATE TABLE IF NOT EXISTS xb_schema_versions (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    subgraph_slug  TEXT NOT NULL DEFAULT 'main',
    version        INT NOT NULL,
    schema_doc     JSONB NOT NULL,                -- 完整 SchemaDoc 快照
    diff           JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 相对上一版本的 SchemaDiff
    change_summary TEXT NOT NULL DEFAULT '',
    published_by   UUID,
    published_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, subgraph_slug, version)
);

CREATE TABLE IF NOT EXISTS xb_schema_drafts (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    subgraph_slug TEXT NOT NULL DEFAULT 'main',
    title         TEXT NOT NULL,
    source        TEXT NOT NULL CHECK (source IN ('MANUAL','AI','RESTORE','TEMPLATE','MERGE')),
    base_version  INT NOT NULL,                   -- 草案基于所属子图的已发布版本
    schema_doc    JSONB NOT NULL,
    ai_context    JSONB NOT NULL DEFAULT '{}'::jsonb,   -- {prompt, dataset_ids, doc_ids, model_name}
    issues        JSONB NOT NULL DEFAULT '[]'::jsonb,   -- ValidationIssue[]
    status        TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PUBLISHED','DISCARDED')),
    created_by    UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS xb_mappings (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        TEXT NOT NULL,
    subgraph_slug    TEXT NOT NULL DEFAULT 'main',
    name             TEXT NOT NULL,
    dataset_id       UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE RESTRICT,
    node_type        TEXT NOT NULL,
    spec             JSONB NOT NULL,                -- MappingSpec（§05.2.5）
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    schema_version   INT NOT NULL,                  -- 定义时数据集 schema_version
    last_snapshot_id BIGINT,                        -- 最近一次物化读取到的湖快照
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, subgraph_slug, name)
);

CREATE TABLE IF NOT EXISTS xb_mapping_runs (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    mapping_id     UUID NOT NULL REFERENCES xb_mappings(id) ON DELETE CASCADE,
    job_id         UUID NOT NULL,
    mode           TEXT NOT NULL CHECK (mode IN ('FULL','INCREMENTAL')),
    from_snapshot  BIGINT,
    to_snapshot    BIGINT,
    nodes_upserted BIGINT NOT NULL DEFAULT 0,
    nodes_deleted  BIGINT NOT NULL DEFAULT 0,
    links_upserted BIGINT NOT NULL DEFAULT 0,
    points_written BIGINT NOT NULL DEFAULT 0,
    rows_rejected  BIGINT NOT NULL DEFAULT 0,
    reject_samples JSONB NOT NULL DEFAULT '[]'::jsonb,   -- 最多 20 条 {row, reason}
    status         TEXT NOT NULL CHECK (status IN ('RUNNING','SUCCEEDED','FAILED')),
    error_message  TEXT,
    started_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at    TIMESTAMPTZ
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xb_subgraphs','xb_schema_heads','xb_schema_versions','xb_schema_drafts','xb_mappings','xb_mapping_runs'
]::regclass[]) AS t;

-- 每个活跃租户默认初始化企业主图（slug='main'）
INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
SELECT tenant_id, 'main', '企业主图', TRUE FROM sec_tenants ON CONFLICT DO NOTHING;

-- 现有模型作为主图版本 1 的基线：已有类型的租户写入 head 与版本快照
INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
SELECT DISTINCT tenant_id, 'main', 1 FROM xb_node_types WHERE subgraph_slug = 'main' ON CONFLICT DO NOTHING;
INSERT INTO xb_schema_versions (tenant_id, subgraph_slug, version, schema_doc, change_summary)
SELECT t.tenant_id, 'main', 1,
       jsonb_build_object(
         'node_types', COALESCE((SELECT jsonb_agg(jsonb_build_object('name', n.name, 'display_name', n.display_name,
              'properties', n.properties, 'agg_rules', n.agg_rules, 'derived_rules', '[]'::jsonb,
              'pos_x', n.pos_x, 'pos_y', n.pos_y))
              FROM xb_node_types n WHERE n.tenant_id = t.tenant_id AND n.subgraph_slug = 'main'), '[]'::jsonb),
         'edge_types', COALESCE((SELECT jsonb_agg(jsonb_build_object('name', e.name, 'src_type_name', e.src_type_name,
              'dst_type_name', e.dst_type_name, 'impact_weight', e.impact_weight, 'cardinality', 'MANY_TO_MANY'))
              FROM xb_edge_types e WHERE e.tenant_id = t.tenant_id AND e.subgraph_slug = 'main'), '[]'::jsonb),
         'mappings', '[]'::jsonb),
       'baseline'
  FROM (SELECT DISTINCT tenant_id FROM xb_node_types WHERE subgraph_slug = 'main') t
ON CONFLICT DO NOTHING;
```

---

## 05.2 模型文档 `SchemaDoc`

模型的唯一交换格式（草案、版本快照、子图合并、AI 输出、模板包、导入导出均使用）：

```rust
pub struct SchemaDoc {
    pub node_types: Vec<NodeTypeSpec>,
    pub edge_types: Vec<EdgeTypeSpec>,
    pub mappings: Vec<MappingSpec>,
}
```

### 05.2.1 `NodeTypeSpec`

| 字段 | 类型 | 规则 |
| :--- | :--- | :--- |
| `name` | String | `^[A-Z][A-Za-z0-9]{0,62}$`，所属子图内唯一 |
| `display_name` | String | 1～64 字符 |
| `description`、`icon`、`color` | String | `color` 为 `#RRGGBB` |
| `title_template` | String | 只允许引用 `{node_key}` 与 `{prop.<name>}` |
| `properties` | `Vec<PropertySpec>` | 见 05.2.2 |
| `agg_rules` | `Vec<AggRuleSpec>` | 见 05.2.3 |
| `derived_rules` | `Vec<DerivedRuleSpec>` | 见 05.2.4 |
| `sync_to_knowledge` | bool | Step 06 §06.5 |
| `pos_x`、`pos_y` | f64 | 画布坐标 |

### 05.2.2 `PropertySpec`（序列化到 `xb_node_types.properties`）

| 字段 | 说明 |
| :--- | :--- |
| `name` | `^[a-z][a-z0-9_]{0,62}$` |
| `display_name`、`description`、`unit` | 展示 |
| `data_type` | A1 `DataType`；仓库已有数据中的 `number` / `string` / `boolean` 在读取时映射为 `FLOAT64` / `STRING` / `BOOLEAN` |
| `kind` | `ATTRIBUTE`（静态属性，存 `xb_nodes.properties`）或 `METRIC`（指标，存 `xb_nodes.metrics` 与 `xb_metric_series`） |
| `required` | 映射时缺失则拒绝该行 |
| `pii_category` | A1 `PiiCategory` |
| `allowed_values` | 可选枚举值 |

### 05.2.3 `AggRuleSpec`（序列化到 `xb_node_types.agg_rules`，“动态聚合”）

```json
{"target_metric": "total_demand", "op": "SUM", "via_link_type": "FULFILLS", "direction": "OUT",
 "source_type_name": "Order", "source_metric": "demand", "filter": "prop.status != 'CANCELED'", "weight_by_link": false}
```

含义：本类型节点的 `target_metric` = 沿 `via_link_type` 关系（`OUT`：本节点为源；`IN`：本节点为目标）连接的 `source_type_name` 节点的 `source_metric` 经 `op` 聚合；`filter` 为 Step 06 表达式（作用于子节点）；`weight_by_link=true` 时 `SUM` / `AVG` 按关系权重加权。`op ∈ AggFn`。

### 05.2.4 `DerivedRuleSpec`（序列化到 `xb_node_types.derived_rules`，“派生指标”）

```json
{"target_metric": "fill_rate", "expression": "if(total_demand > 0, inventory / total_demand, null)", "enabled": true}
```

表达式语法见 Step 06 §06.1，只能引用同一节点的指标与 `prop.<属性>`。

### 05.2.5 `MappingSpec`（数据集 → 节点 / 关系 / 指标）

```json
{
  "name": "orders_to_order_nodes",
  "dataset": "demo_orders",
  "node_type": "Order",
  "key_column": "id",
  "title": "订单 {id}",
  "filter": "status != 'TEST'",
  "attributes": {"status": "status", "customer": "customer_name"},
  "metrics": {"demand": "amount"},
  "metric_time_column": "updated_at",
  "geo": {"lng_column": null, "lat_column": null},
  "links": [{"link_type": "FULFILLS", "direction": "IN", "target_type": "Warehouse", "target_key_column": "warehouse_code",
             "weight_column": null, "create_missing_target": false}],
  "on_source_delete": "KEEP",
  "mode": "INCREMENTAL"
}
```

- `title` 是模板：`{列名}` 引用当前行。
- `metrics` 有 `metric_time_column` 时写时序点（`source_tag=SYNC`），同时以每个键最大时间的值刷新最新快照；没有时间列时只刷新快照并以物化时刻写一个点。
- `links[].direction`：`OUT` 表示本节点为源，`IN` 表示本节点为目标。
- `create_missing_target=true` 时，目标节点不存在则以 `target_key_column` 值创建最小节点（标题等于键）。
- `on_source_delete`：`KEEP`（保留节点）或 `DELETE`（源行删除时删除节点）。
- `mode`：`FULL` 每次全量；`INCREMENTAL` 基于 DuckLake 快照变更集（Step 06 §06.2）。

---

## 05.3 校验（`studio/validate.rs`）

`validate(doc, ctx) -> Vec<ValidationIssue>`，`ValidationIssue { severity: ERROR|WARN, path: "node_types[2].agg_rules[0].source_metric", code, message }`。存在 `ERROR` 时不允许发布。

| # | 规则 | code |
| :--- | :--- | :--- |
| 1 | 命名格式、唯一性、保留字（`id`、`tenant_id`、`node_key`、`title`、`metrics`、`properties`） | `NAME_INVALID` / `NAME_TAKEN` |
| 2 | 关系类型的源 / 目标类型存在于当前子图或企业主图 `main` 中 | `TYPE_NOT_FOUND` |
| 3 | `impact_weight ∈ (0, 1]` | `WEIGHT_OUT_OF_RANGE` |
| 4 | 聚合规则引用的关系类型存在且方向与源类型匹配；`source_metric` 是源类型的 `METRIC` 属性；`target_metric` 是本类型的 `METRIC` 属性 | `AGG_REF_INVALID` |
| 5 | 派生规则表达式可解析、只引用本类型指标与属性、类型检查通过 | `EXPR_INVALID` |
| 6 | 指标依赖图无环：节点为 `(type, metric)`，边来自聚合规则（源 → 目标）与派生规则（引用 → 目标） | `CYCLE_DETECTED` |
| 7 | 同一 `(type, metric)` 只能被一条聚合规则或一条派生规则产出 | `METRIC_MULTIPLE_PRODUCERS` |
| 8 | 映射：数据集存在且 `READY`；引用列存在；`key_column` 非空率 = 100%（基于列画像）；列类型与属性 `data_type` 可转换 | `MAPPING_INVALID` |
| 9 | 基数提示：`ONE_TO_MANY` 关系的现有数据中存在一个目标对应多个源时给出 `WARN` | `CARDINALITY_VIOLATION` |
| 10 | 破坏性变化（与所属子图当前版本比较）：删除仍有节点的类型、删除仍有关系的关系类型、修改已有数据属性的 `data_type` 为不可转换类型 | `BREAKING_CHANGE`（`WARN`，发布时需 `force=true`） |

`POST /api/xblue/schema/validate`（`STUDIO_EDIT`）对任意 `SchemaDoc` 返回问题列表；画布在每次编辑后防抖 500 毫秒调用。

---

## 05.4 草案、发布、差异、回滚与多子图协同

### 05.4.1 发布事务（`studio_repo.rs::publish`）

每个子图拥有独立的乐观锁与版本链（查询参数缺省时 `subgraph = "main"`）：

```text
BEGIN（租户事务）
  SELECT pg_advisory_xact_lock(hashtext('xb_schema:' || $tenant || ':' || $subgraph))
  SELECT current_version FROM xb_schema_heads WHERE tenant_id = $tenant AND subgraph_slug = $subgraph FOR UPDATE
  若 draft.base_version ≠ current_version → 409 VERSION_CONFLICT（响应附带最新版本号，前端提示“基于最新版本重新合并”）
  validate(draft.schema_doc)；存在 ERROR → 400 SCHEMA_INVALID（附问题列表）
  diff = diff(current_doc, draft.schema_doc)；存在 BREAKING_CHANGE 且 force=false → 409 SCHEMA_BREAKING_CHANGE
  按 diff 写 xb_node_types / xb_edge_types / xb_mappings（限定 subgraph_slug = $subgraph；主图删除类型时 force=true 且 cascade=true 才删除其节点）
  INSERT xb_schema_versions(subgraph_slug, version = current + 1, schema_doc, diff, change_summary)
  UPDATE xb_schema_heads SET current_version = current + 1 WHERE tenant_id = $tenant AND subgraph_slug = $subgraph
  UPDATE xb_schema_drafts SET status = 'PUBLISHED'
  写血缘（DATASET → NODE_TYPE，来自主图映射）、审计 SCHEMA_PUBLISH
COMMIT
```

- **运行时边界**：企业主图（`subgraph_slug = 'main'`）是生产图实例（`xb_nodes`、`xb_links`）、动态聚合、推演与领域对象 API 的唯一权威模式，所有运行时对 `xb_node_types`、`xb_edge_types`、`xb_mappings` 的读取固定过滤 `subgraph_slug = 'main'`；主图发布后自动刷新 `DomainGraphqlRegistry`（§05.6），并对主图新增或变化的映射入队 `MAP_MATERIALIZE`（`mode=FULL`）、对变化的聚合 / 派生规则入队 `GRAPH_AGGREGATE`。
- **部门子图**（`subgraph_slug != 'main'`）：只承载隔离建模、版本演进与映射预览（`POST /api/xblue/mappings/{id}/preview`），不写入生产 `xb_nodes`，不阻塞主图或其他部门子图的并发编辑；经 §05.4.5 合并入 `main` 后统一下发生效。

### 05.4.2 差异（`studio/diff.rs`）

`SchemaDiff { node_types: {added, removed, changed: [{name, fields: [{path, before, after}]}]}, edge_types: {...}, mappings: {...} }`；比较以 `name` 为键，字段级逐项比较（数组按 `name` 或 `target_metric` 对齐）。`GET /api/xblue/schema/versions/{a}/diff/{b}?subgraph=` 返回同一子图任意两版本差异。

### 05.4.3 整包保存

`POST /api/xblue/schema?subgraph=`（仓库已有路由，缺省 `subgraph=main`）接收 `SchemaDoc`（`node_types`、`edge_types`，可选 `mappings`），语义为“以该子图当前版本为基创建草案并立即发布”，主图需要 `STUDIO_PUBLISH`，非主图子图需要 `STUDIO_EDIT`；请求可带 `base_version`（缺省取当前版本）。`GET /api/xblue/schema?subgraph=` 返回目标子图当前版本的 `SchemaDoc` 与 `version`。

### 05.4.4 回滚

`POST /api/xblue/schema/versions/{v}/restore?subgraph=` 以版本 `v` 的快照创建 `source=RESTORE` 的草案（`base_version` = 当前版本），走正常发布流程。

### 05.4.5 多子图隔离建模与跨部门本体对齐合并（`studio/merge.rs`）

企业多业务团队（仓储、制造、财务、客服）各自创建部门子图独立建模，成熟后对齐合并至企业主图 `main`：

1. **子图管理**：
   - `POST /api/xblue/subgraphs`（`STUDIO_EDIT`）：`{slug, display_name, description?, owner_dept?, seed_from_main?: bool}`；`seed_from_main=true` 时可选择主图的一组 `node_types` 作为初始上下文拷贝。`slug="main"` 为系统保留，不可删除。
   - Step 13 启用后，非主图子图的编辑受 `owner_dept` ABAC 约束（本部门 `STUDIO_EDIT` 可独立维护与发布子图版本，合并入 `main` 需 `STUDIO_PUBLISH`）。
2. **对齐预览**（`POST /api/xblue/subgraphs/{slug}/merge-preview`，`STUDIO_EDIT`）：
   - 将子图当前已发布 `SchemaDoc` 与 `main` 当前 `SchemaDoc` 比对，输出 `MergePreview { alignments, conflicts, added_node_types, added_edge_types, added_mappings }`：
   - **同义实体类型识别**：对子图每个 `NodeTypeSpec` 与主图类型计算综合对齐分 `0.4 × 名称/显示名归一化匹配 + 0.4 × 属性名集合 Jaccard + 0.2 × 类型描述向量余弦相似度`；得分 ≥ 0.65 输出为候选对齐 `{sub_type, main_type, score, suggested_strategy}`（同名时得分 = 1.0）。
   - **冲突检测**：同名或对齐目标上的属性 `data_type` 不兼容（`PROPERTY_TYPE_CONFLICT`）、同名指标产出规则不一致（`RULE_CONFLICT`）、同名关系端点或 `cardinality` 冲突（`EDGE_CONFLICT`）、映射名称重名（`MAPPING_NAME_CONFLICT`）。
3. **合并执行**（`POST /api/xblue/subgraphs/{slug}/merge`，`STUDIO_PUBLISH`）：
   - 请求：`{base_main_version?, default_strategy: SubgraphMergeStrategy, resolutions?: [{sub_type, main_type, strategy, bridge_edge_name?}], change_summary}`。
   - 三种合并策略（`SubgraphMergeStrategy`）：
     - `REUSE_MAIN`：直接复用主图已有同名类型定义，仅导入子图新增的非冲突关系与映射；
     - `EXTEND_PROPS`：保留主图已有属性与规则，把子图中新增的属性、指标、派生/聚合规则与映射并入主图目标类型（存在未消解类型冲突时返回 409 `SCHEMA_BREAKING_CHANGE`）；
     - `BRIDGE_LINK`：把子图类型保留为带领域前缀的独立类型（或原名），并在其与主图目标类型间自动建立 `ONE_TO_ONE` 桥接关系类型（默认 `SAME_ENTITY_AS` 或 `bridge_edge_name`）。
   - 合并在 `main` 的发布事务（§05.4.1）内原子提交，生成 `source=MERGE` 的记录与新的主图 `version`，并自动刷新领域 GraphQL Schema 与映射物化队列。

---

## 05.5 AI 草案

`POST /api/xblue/drafts/ai?subgraph=`（`STUDIO_EDIT`）：`{title, dataset_ids?: [], doc_ids?: [], description?: ""}`，三者至少一项。

1. **确定性预分析**（`axpoint-analytics::profiling`）：
   - 每个数据集：主键候选（非空且唯一的列，优先 `id` / `*_id` / `*_code` / `*_no`）、数值列（候选指标）、时间列、分类列（基数 ≤ 50）。
   - 外键推断：列名形如 `<x>_id` / `<x>_code` 或与另一数据集主键同名，且 `|A.col ∩ B.pk| / |A.col distinct| ≥ 0.9`（DuckDB 内部查询，单次抽样 100 000 行）→ 候选关系 `A → B`。
   - 基数：`A.col` 去重数 / `A` 行数 ≥ 0.99 → `ONE_TO_ONE`，否则 `MANY_TO_ONE`（A 指向 B）。
2. **文档抽取**：`doc_ids` 的实体与 `RELATES` 边（Step 03 超图）按实体类型分组，作为类型与关系候选。
3. **大模型生成**：输入预分析结果、实体分组、描述，要求输出 `SchemaDoc` JSON（含映射），JSON Schema 约束输出结构；`name` 采用英文 PascalCase，`display_name` 采用中文。
4. **后处理**：执行 §05.3 校验，把问题写入 `issues`；对模型虚构的列（不在数据集中）直接移除并记录 `WARN`。
5. 写 `xb_schema_drafts(subgraph_slug, source=AI, ai_context)`，返回草案与问题列表。

评测：`axpoint-llm/tests/schema_draft_eval.rs` 以演示供应链 3 个数据集为输入，使用录制响应断言：生成 `Factory`、`Warehouse`、`Order` 三个类型，`SUPPLIES` / `FULFILLS` 两个关系方向正确，映射主键列正确。

---

## 05.6 领域对象自动发布与动态 GraphQL 接口（`xblue_graphql.rs`）

主图 `SchemaDoc` 一旦发布，无需手写胶水代码，即刻按最新模型结构暴露强类型领域对象查询接口：

1. **动态 GraphQL Schema（`async-graphql::dynamic`）**：
   - `DomainGraphqlRegistry` 按 `(tenant_id, main_schema_version)` LRU 缓存编译好的 `dynamic::Schema`（上限 64 个租户实例），主图版本推进时旧实例自动淘汰；统一开启 `.limit_depth(6)` 与 `.limit_complexity(200)` 防止深层嵌套放大查询。
   - 每个 `NodeTypeSpec` 编译为同名 GraphQL `Object`：固定字段 `id: ID!`、`nodeKey: String!`、`title: String!`、`updatedAt: String!`，所有 `PropertySpec`（含 `ATTRIBUTE` 与 `METRIC`）按 `data_type` 编译为强类型字段（`STRING → String`、`INT64 → Int`、`FLOAT64/DECIMAL → Float`、`BOOLEAN → Boolean`、`DATE/TIMESTAMP → String`、`JSON/LIST/STRUCT → JSON`）。
   - 每个 `EdgeTypeSpec` 在源类型上生成正向遍历字段（例 `Warehouse.fulfillsOrders(limit: Int = 50): [Order!]!`），在目标类型上生成反向遍历字段（例 `Order.fulfilledByWarehouses(limit: Int = 50): [Warehouse!]!`）；解析器使用按请求生命周期批处理的 `DataLoader` 调用 `GraphQueryPort`，消除 N+1 查询。
   - `Query` 根对象自动为每个类型生成单对象查询 `<camelName>(nodeKey: String!): <Name>`（例 `warehouse(nodeKey: "W-1")`）与分页过滤查询 `<camelName>List(q: String, filter: JSON, limit: Int = 50, cursor: String): <Name>Connection!`。
2. **强类型领域对象 REST 接口**：
   - `GET /api/xblue/objects`：返回主图已发布类型目录、字段签名与示例查询。
   - `GET /api/xblue/objects/{type_name}?q=&filter=&limit=&cursor=`：按已发布类型查询对象集合（未发布类型返回 404 `TYPE_NOT_FOUND`）。
   - `GET /api/xblue/objects/{type_name}/{node_key}?include_neighbors=false`：按业务主键读取单个强类型领域对象（`attributes` 与 `metrics` 均按模型类型转换校验，可选附带 1 跳强类型关系）。
3. **安全与生态联动**：
   - GraphQL 与对象 REST 解析器统一携带 `AuthPrincipal`：PII 属性遵循默认脱敏规则（同 Step 04 §04.3），Step 13 启用后自动执行节点类型 Cedar 鉴权与列级脱敏策略。
   - Step 09 发布中心可直接将任意已发布 `NodeType` 作为 `OBJECT_TYPE` 发布至 `/api/published/{env}/{slug}`、OpenAPI 3.1 与 MCP 工具。

---

## 05.7 路由（`/api/xblue` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /subgraphs`、`POST /subgraphs`、`DELETE /subgraphs/{slug}` | `DATA_READ` / `STUDIO_EDIT` / `STUDIO_PUBLISH` |
| `POST /subgraphs/{slug}/merge-preview`、`POST /subgraphs/{slug}/merge` | `STUDIO_EDIT` / `STUDIO_PUBLISH` |
| `GET /schema`、`POST /schema`（均支持 `?subgraph=`，缺省 `main`） | `DATA_READ` / `STUDIO_PUBLISH`（非主图为 `STUDIO_EDIT`） |
| `POST /schema/validate` | `STUDIO_EDIT` |
| `GET /schema/versions`、`GET /schema/versions/{v}`、`GET /schema/versions/{a}/diff/{b}`、`POST /schema/versions/{v}/restore`（支持 `?subgraph=`） | `DATA_READ` / `DATA_READ` / `DATA_READ` / `STUDIO_EDIT` |
| `GET /drafts`、`POST /drafts`、`GET /drafts/{id}`、`PUT /drafts/{id}`、`POST /drafts/{id}/discard`（支持 `?subgraph=`） | `STUDIO_EDIT` |
| `POST /drafts/ai`（支持 `?subgraph=`） | `STUDIO_EDIT` |
| `POST /drafts/{id}/publish`（`{change_summary, force?, cascade?}`） | 主图 `STUDIO_PUBLISH`；部门子图 `STUDIO_EDIT` |
| `GET /mappings`、`GET /mappings/{id}/runs`、`POST /mappings/{id}/run`（`{mode}`）、`POST /mappings/{id}/preview`（前 50 行映射结果与拒绝原因，不写库） | `DATA_READ` / `DATA_READ` / `DATA_SYNC` / `STUDIO_EDIT` |
| `POST /graphql`、`GET /graphql/sdl` | `DATA_READ` |
| `GET /objects`、`GET /objects/{type_name}`、`GET /objects/{type_name}/{node_key}` | `DATA_READ` |

`PUT /drafts/{id}` 整体替换草案文档并重新校验；草案的创建者与 `STUDIO_PUBLISH` 持有者可编辑。

---

## 05.8 前端 `XBlueStudioPage`

| 组件 | 功能 |
| :--- | :--- |
| `StudioSubgraphBar` | 顶部子图切换器（`企业主图 main` + 各部门子图）、新建部门子图弹窗、“对比并合并到主图”入口 |
| `SubgraphMergeDialog` | 展示同义类型对齐得分、属性/规则冲突项，逐项选择 `REUSE_MAIN` / `EXTEND_PROPS` / `BRIDGE_LINK` 后执行合并 |
| `StudioCanvas` | `@xyflow/react` 画布：节点类型卡片（图标、颜色、属性数、指标数）、关系连线（标签显示名称与 `1:N` 等基数）；拖拽连线创建关系类型；`elkjs` 一键布局；坐标随草案保存 |
| `NodeTypeCard` / `PropertyTable` | 属性增删改、`kind`、`data_type`、单位、PII |
| `EdgeTypeEditor` | 名称、方向、基数、`impact_weight`、传导参数（Step 10 使用） |
| `RuleEditor` | 聚合规则表单（关系、方向、源类型、源指标、聚合函数、过滤）与派生规则表达式编辑（语法高亮、实时校验、引用补全） |
| `MappingEditor` | 选择数据集 → 列到属性 / 指标 / 关系的拖拽映射 → 预览前 50 行结果与拒绝原因 |
| `DomainApiDrawer` | 实时查看当前已发布主图的 GraphQL SDL、对象 REST 路径，并内置交互式 GraphQL 查询试运行面板 |
| `IssueList` | 校验问题列表，点击定位到画布元素 |
| `VersionPanel` / `DiffView` | 版本列表、任意两版本差异（增加绿、删除红、变化黄）、恢复为草案 |
| `AiDraftWizard` | 选择数据集 / 文档 / 输入描述 → 生成草案 → 在画布中审阅 |

页面顶部始终显示“当前子图 · 当前编辑草案 · 基于版本 vN”；版本冲突时提示并提供“查看差异”。

Playwright：`e2e/xbluestudio.spec.ts` 打开 `/studio`，看到 `Warehouse`、`Order` 卡片与 `FULFILLS` 连线，打开版本面板至少 1 个版本，打开“领域接口”抽屉可见 GraphQL SDL。

---

## 05.9 Step 05 验收

`scripts/seed/05-studio.sh`：在 `Warehouse` 类型声明指标属性 `inventory`、`capacity`、`total_demand`、`fill_rate`，在 `Order` 类型声明指标 `demand`、属性 `status` 与 `customer`；在 `Warehouse` 增加聚合规则 `total_demand = SUM(Order.demand via FULFILLS OUT)` 与派生规则 `fill_rate = if(total_demand > 0, inventory / total_demand, null)`；增加映射 `orders_to_order_nodes`（演示表 `demo_orders` 含 `warehouse_code` 列，由 Step 02 种子生成：`id % 2 = 0 → W-1`，否则 `W-2`）；发布主图模型。输出 `SCHEMA_VERSION`、`ORDER_MAPPING_ID`。

`scripts/gate/step05.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain studio && cargo test -p axpoint-db studio_repo && cargo test -p axpoint-llm schema_draft_eval)

# 环检测
CYCLIC=$(api GET /api/xblue/schema | jq '.node_types |= map(if .name=="Warehouse" then .derived_rules += [{"target_metric":"inventory","expression":"fill_rate * 2","enabled":true}] else . end)')
expect_json "cycle" "$(api POST /api/xblue/schema/validate "$CYCLIC")" '[.issues[].code] | index("CYCLE_DETECTED") != null'

# 并发发布冲突
D1=$(api POST /api/xblue/drafts "{\"title\":\"a\",\"schema_doc\":$(api GET /api/xblue/schema | jq .)}" | jq -r .id)
D2=$(api POST /api/xblue/drafts "{\"title\":\"b\",\"schema_doc\":$(api GET /api/xblue/schema | jq .)}" | jq -r .id)
expect_code "publish-1" 200 POST "/api/xblue/drafts/$D1/publish" '{"change_summary":"a"}'
expect_json "publish-2-conflict" "$(api POST "/api/xblue/drafts/$D2/publish" '{"change_summary":"b"}')" '.error_code == "VERSION_CONFLICT"'

# 版本与差异
V=$(api GET /api/xblue/schema | jq -r .version)
expect_json "diff" "$(api GET "/api/xblue/schema/versions/1/diff/$V")" '(.node_types.changed | length) >= 1'

# 多子图隔离建模与合并入主图
api POST /api/xblue/subgraphs '{"slug":"logistics","display_name":"物流子图","owner_dept":"logistics"}' >/dev/null
SUB_DOC='{"node_types":[{"name":"Shipment","display_name":"运单","properties":[{"name":"carrier","display_name":"承运商","data_type":"STRING","kind":"ATTRIBUTE","required":false,"pii_category":"NONE"}],"agg_rules":[],"derived_rules":[],"sync_to_knowledge":false,"pos_x":420,"pos_y":160}],"edge_types":[{"name":"SHIPS_ORDER","src_type_name":"Shipment","dst_type_name":"Order","impact_weight":0.7,"cardinality":"MANY_TO_ONE"}],"mappings":[]}'
SD=$(api POST '/api/xblue/drafts?subgraph=logistics' "{\"title\":\"ship\",\"schema_doc\":$SUB_DOC}" | jq -r .id)
expect_code "subgraph-publish" 200 POST "/api/xblue/drafts/$SD/publish" '{"change_summary":"init shipment"}'
expect_json "subgraph-isolated" "$(api GET /api/xblue/schema)" '([.node_types[].name] | index("Shipment")) == null'
expect_json "subgraph-preview" "$(api POST /api/xblue/subgraphs/logistics/merge-preview '{}')" '(.added_node_types | index("Shipment")) != null'
expect_code "subgraph-merge" 200 POST /api/xblue/subgraphs/logistics/merge '{"default_strategy":"EXTEND_PROPS","change_summary":"merge logistics"}'
expect_json "subgraph-merged-main" "$(api GET /api/xblue/schema)" '([.node_types[].name] | index("Shipment")) != null'

# 破坏性变化需要 force
BREAK=$(api GET /api/xblue/schema | jq '.node_types |= map(select(.name != "Order"))')
D3=$(api POST /api/xblue/drafts "{\"title\":\"break\",\"schema_doc\":$BREAK}" | jq -r .id)
expect_json "breaking" "$(api POST "/api/xblue/drafts/$D3/publish" '{"change_summary":"x"}')" '.error_code == "SCHEMA_BREAKING_CHANGE" or .error_code == "SCHEMA_INVALID"'
api POST "/api/xblue/drafts/$D3/discard" >/dev/null

# AI 草案（录制响应）
expect_json "ai-draft" "$(api POST /api/xblue/drafts/ai "{\"title\":\"ai\",\"dataset_ids\":[\"$DEMO_ORDERS_DATASET_ID\"]}")" '(.schema_doc.node_types | length) >= 1'

# 自动发布的动态 GraphQL 与强类型领域对象 API
expect_json "domain-graphql" "$(api POST /api/xblue/graphql '{"query":"{ warehouse(nodeKey: \"W-1\") { title inventory fulfillsOrders(limit: 5) { nodeKey } } }"}')" \
  '.data.warehouse.title == "华东仓" and .data.warehouse.inventory == 500 and (.data.warehouse.fulfillsOrders | length) >= 2'
expect_json "domain-objects" "$(api GET /api/xblue/objects/Warehouse/W-1?include_neighbors=true)" \
  '.type_name == "Warehouse" and .node_key == "W-1" and .metrics.inventory == 500'

gate_pass step05
```

**通过标准**：最后一行 `GATE step05 PASS`。

---

# Step 06：表达式引擎、映射物化、派生与动态聚合、知识同步

## 06.0 目标、前置与契约

- **目标**：确定性的表达式引擎；映射全量与增量物化（幂等、可预览、拒绝行可追溯）；派生指标与多级动态聚合在指标变化后自动、正确、无竞争地刷新；业务节点同步为知识文档、文档实体反哺节点候选。
- **前置**：Step 05 通过。
- **迁移**：无（使用 0004、0006）。
- **契约清单**：`axpoint-domain::expr`（`parse`、`typecheck`、`eval`）；`axpoint-domain::rollup`（依赖图、传播计划）；`JobKind::{MAP_MATERIALIZE, GRAPH_AGGREGATE, KNOWLEDGE_SYNC}` 的载荷结构；`docs/API_CONTRACT.md` 的 `/api/xblue/expr/*`、`/api/xblue/aggregate/*`、`/api/xblue/knowledge/*`。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `axpoint-core/crates/axpoint-domain/src/expr/{mod.rs, lexer.rs, parser.rs, types.rs, eval.rs, functions.rs}` | 表达式引擎（§06.1） |
| 新建 | `axpoint-core/crates/axpoint-domain/src/rollup/{mod.rs, plan.rs}` | 指标依赖图与传播计划（§06.3） |
| 新建 | `axpoint-core/crates/axpoint-db/src/{mapping_exec.rs, aggregate_exec.rs, knowledge_sync.rs}` | 执行器 |
| 新建 | `axpoint-core/crates/axpoint-server/src/workers/{mapping_worker.rs, aggregate_worker.rs, knowledge_worker.rs}` | 任务处理与外发订阅 |
| 修改 | `axpoint-core/crates/axpoint-server/src/background.rs` | 注册三个任务处理器与 `dataset.synced`、`node.metrics_changed`、`node.upserted` 订阅者 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/xblue.rs` | §06.6 路由 |
| 新建 | `axpoint-core/crates/axpoint-domain/tests/{expr.rs, rollup.rs}`、`tests/expr_cases.json` | 测试 |
| 新建 | `scripts/gate/step06.sh` | 门禁 |

---

## 06.1 表达式引擎（`axpoint-domain::expr`）

### 06.1.1 语法（EBNF）

```ebnf
expr        = or_expr ;
or_expr     = and_expr , { ( "or" | "||" ) , and_expr } ;
and_expr    = not_expr , { ( "and" | "&&" ) , not_expr } ;
not_expr    = [ "not" | "!" ] , cmp_expr ;
cmp_expr    = add_expr , [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) , add_expr ] ;
add_expr    = mul_expr , { ( "+" | "-" ) , mul_expr } ;
mul_expr    = unary , { ( "*" | "/" | "%" ) , unary } ;
unary       = [ "-" ] , power ;
power       = primary , [ "^" , unary ] ;               (* 右结合 *)
primary     = number | string | "true" | "false" | "null"
            | ident_ref | call | "(" , expr , ")" ;
ident_ref   = ident | "prop" , "." , ident ;            (* 指标名或 prop.<属性> *)
call        = ident , "(" , [ expr , { "," , expr } ] , ")" ;
ident       = letter , { letter | digit | "_" } ;
number      = digit , { digit } , [ "." , digit , { digit } ] , [ ( "e" | "E" ) , [ "+" | "-" ] , digit , { digit } ] ;
string      = "'" , { char - "'" | "''" } , "'" ;
```

实现为 Pratt 解析器，绑定力（由低到高）：`or`=1、`and`=2、`not`=3（前缀）、比较=4（不可链式，`a < b < c` 报错）、`+ -`=5、`* / %`=6、一元 `-`=7、`^`=8（右结合）。

### 06.1.2 类型与求值

- 值类型：`Number(f64)`、`Text(String)`、`Bool`、`Null`。
- 类型检查（发布时）：指标为 `Number`；属性按 `PropertySpec.data_type`（数值类 → `Number`，`STRING` → `Text`，`BOOLEAN` → `Bool`，其余不可引用）；运算符与函数签名不匹配 → `EXPR_INVALID`。
- 空值传播：算术与比较遇 `Null` 结果为 `Null`；`and` / `or` 按三值逻辑；`coalesce`、`if`、`is_null` 处理空值。
- 除以 0、`%` 0、`sqrt` 负数、`log` 非正数、结果非有限 → 结果 `Null`，并在求值报告中记录 `DIV_BY_ZERO` / `DOMAIN_ERROR`（不写入非有限值，符合 A1 §7）。
- 限制：表达式长度 ≤ 2 000 字符，AST 深度 ≤ 64，函数参数 ≤ 16。

### 06.1.3 内置函数

| 函数 | 签名 |
| :--- | :--- |
| `abs`、`sqrt`、`exp`、`ln`、`log10`、`floor`、`ceil` | `(Number) -> Number` |
| `round` | `(Number, Number=0) -> Number`（银行家舍入不采用，按“四舍五入，远离零”） |
| `pow` | `(Number, Number) -> Number` |
| `min`、`max` | `(Number, Number, ...) -> Number`（忽略 `Null`，全为 `Null` 时 `Null`） |
| `clamp` | `(Number, lo, hi) -> Number` |
| `if` | `(Bool, T, T) -> T` |
| `coalesce` | `(T, T, ...) -> T` |
| `is_null` | `(T) -> Bool` |
| `contains`、`starts_with` | `(Text, Text) -> Bool` |
| `lower`、`upper` | `(Text) -> Text` |
| `safe_div` | `(Number, Number, Number=null) -> Number`（分母为 0 返回第三参数） |

**测试**（`tests/expr.rs` + `expr_cases.json` ≥ 150 条）：优先级与结合性、三值逻辑、空值传播、错误位置（列号）、深度与长度限制、每个函数的边界值；属性测试（`proptest`）：随机生成的合法 AST 打印后再解析得到同一 AST。

---

## 06.2 映射物化（`MAP_MATERIALIZE`）

载荷：`{mapping_id, mode: "FULL"|"INCREMENTAL"}`；幂等键 `<mapping_id>:<目标快照>`。

### 06.2.1 读取

- `FULL`：`SELECT <引用列> FROM lake."tenant_<id>".<t> WHERE <filter>`（`filter` 由 §06.1 引擎解析后翻译为 DuckDB SQL，只允许列引用、字面量、比较与布尔运算）。
- `INCREMENTAL`：`from = xb_mappings.last_snapshot_id`，`to = ds_datasets.last_snapshot_id`；用 `ducklake_table_changes('lake', 'tenant_<id>', '<t>', from + 1, to)` 读取变更集（`change_type ∈ insert / update_postimage / delete`）；`from` 为空或快照已被过期清理时自动退化为 `FULL`。
- 读取通过 `axpoint-analytics` 的内部执行入口 `run_internal(tenant, sql)`（只接受代码生成的 SQL，不对外暴露），按 5 000 行分块流式处理。

### 06.2.2 写入

每块在一个租户事务中：

1. 行校验：`key_column` 为空、`required` 属性缺失、类型转换失败的行计入 `rows_rejected`，前 20 条写 `reject_samples`。
2. 节点：Step 03 批量 upsert（`source_dataset_id`、`source_row_key = key`），`title` 按模板渲染，属性写 `properties`，指标按 §05.2.5 规则写快照与时序点。
3. 关系：按 `target_type + target_key_column` 批量解析目标节点 ID；`create_missing_target=true` 时先批量创建；关系 upsert，`weight_column` 缺省时取关系类型 `impact_weight`。
4. 删除：`delete` 变更且 `on_source_delete=DELETE` 时删除节点；`FULL` 模式下，`on_source_delete=DELETE` 删除本映射来源（`source_dataset_id` 相同）但本次未出现的节点。
5. 外发：`node.upserted`、`node.metrics_changed`（每块合并一条）。

全部块完成后写 `xb_mappings.last_snapshot_id = to`、`xb_mapping_runs(SUCCEEDED)`。中途失败：已提交的块保持（节点 upsert 幂等），`last_snapshot_id` 不推进，重试从头执行同一变更集。

### 06.2.3 触发

- `dataset.synced` 订阅者：对 `enabled`、`subgraph_slug = 'main'` 且 `dataset_id` 匹配的映射入队 `INCREMENTAL`。
- 手动：`POST /api/xblue/mappings/{id}/run`（仅 `subgraph_slug = 'main'`；部门子图使用预览接口）。
- 发布模型后：§05.4.1（主图发布后对受影响映射触发重算）。
- 映射引用列因数据集 Schema 变化消失（`schema_version` 变化后校验失败）时，运行失败并把映射标为 `enabled=false`，`xb_mapping_runs.error_message` 写明缺失列。

---

## 06.3 派生指标与动态聚合（`GRAPH_AGGREGATE`）

### 06.3.1 依赖图与传播计划（`axpoint-domain::rollup`）

- 由主图（`subgraph_slug = 'main'`）当前已发布模型构造指标依赖图 `G`：顶点 `(type, metric)`，边 `源 → 目标`（聚合规则：子类型指标 → 父类型目标指标；派生规则：被引用指标 → 目标指标）。发布时已保证无环（§05.3 规则 6）。
- 拓扑序 `order(G)` 缓存于内存，模型版本变化时重建。
- 传播计划：给定一组“脏”顶点实例 `(node_id, metric)`，按拓扑序逐层处理：
  1. 对每个脏实例，找出以其 `(type, metric)` 为源的规则；
  2. 派生规则 → 同一节点的目标指标标脏；
  3. 聚合规则 → 沿 `via_link_type` 反向找到父节点（`direction=OUT` 表示父节点为关系源，子节点为目标，则从子节点沿入边找父；`IN` 相反），父节点目标指标标脏；
  4. 同一层内的父节点去重后一次性重算。

### 06.3.2 执行（`aggregate_exec.rs`）

载荷：`{node_ids: [...], metrics: [...]}` 或 `{full_type: "Warehouse"}`；幂等键为载荷哈希。

```text
pg_advisory_lock(hashtext('xb_agg:' || tenant))      -- 会话级锁，同租户聚合串行，避免两次传播交错写入
for layer in plan.layers():
    派生：批量读取节点 metrics → expr::eval → 批量写快照与时序点（source_tag = DERIVED）
    聚合：对每条规则执行一条 SQL：
        SELECT p.id AS parent_id, <AGG>(<子指标>[* l.weight]) AS v
        FROM xb_links l JOIN xb_nodes c ON c.id = l.<子端> JOIN xb_nodes p ON p.id = l.<父端>
        WHERE l.link_type = $1 AND c.type_name = $2 AND p.id = ANY($3) [AND <filter 翻译后>]
        GROUP BY p.id
      无子节点的父节点：SUM / COUNT 写 0，其余写 Null（从 metrics 中移除该键）
      写快照与时序点（source_tag = AGGREGATION），值未变化（相对误差 < 1e-12）的不写、不再向上传播
pg_advisory_unlock(...)
外发 node.metrics_changed（只含实际变化的节点）
```

- `AggFn` 到 SQL：`SUM`→`sum`、`AVG`→`avg`、`COUNT`→`count`、`COUNT_DISTINCT`→`count(DISTINCT ...)`、`MIN`/`MAX`、`MEDIAN`→`percentile_cont(0.5) WITHIN GROUP (ORDER BY ...)`、`LAST`→子节点中 `properties.__metric_ts.<metric>` 最新者的值。
- 子指标从 `xb_nodes.metrics->>'<metric>'` 读取并转 `double precision`；`filter` 翻译为对 `c.metrics` / `c.properties` 的 SQL 表达式（翻译器只支持列引用、字面量、比较、布尔运算、`is_null`；含其他函数的过滤在 Rust 中逐行求值）。
- 由聚合与派生产生的 `node.metrics_changed` 事件带 `origin: "AGGREGATE"`，订阅者据此跳过对同一传播的重复入队（传播已在本次任务内完成到顶层）。

### 06.3.3 触发

- `node.metrics_changed`（`origin ≠ AGGREGATE`）订阅者：以事件中的节点与变化指标入队。
- 关系增删（`xb_links` 写入路径）：两端节点入队。
- `POST /api/xblue/aggregate/recompute {type_name?}`：全量重算（`STUDIO_EDIT`）。

**测试**（`tests/rollup.rs` 与 `axpoint-db` 集成测试）：

| 场景 | 期望 |
| :--- | :--- |
| 3 级树（工厂 ← 仓库 ← 订单），修改 1 个订单 `demand` | 所属仓库 `total_demand`、`fill_rate` 与工厂级聚合同时正确；未受影响的仓库不产生写入 |
| 两个并发任务分别修改同一仓库下两个订单 | 最终仓库值等于两者都生效后的值（咨询锁串行化） |
| 删除仓库下全部订单关系 | `total_demand = 0`，`fill_rate = null` |
| 加权聚合 | 与手工计算一致 |
| `LAST` | 取时间戳最新子节点 |

---

## 06.4 求值报告与可解释性

`GET /api/xblue/nodes/{id}/explain/{metric}` 返回该指标的来源：`{producer: "AGG"|"DERIVED"|"SYNC"|"MANUAL"|"ACTION", rule, inputs: [{node_id, title, metric, value}], computed_at, warnings: ["DIV_BY_ZERO"]}`，前端在节点检查器中以“计算链”展示（逐级展开）。

---

## 06.5 知识 ↔ 节点同步（`KNOWLEDGE_SYNC`）

| 方向 | 规则 |
| :--- | :--- |
| 节点 → 知识 | `sync_to_knowledge=true` 的类型，节点 upsert 后入队（幂等键 `<node_id>:<row_version>`，同一节点 60 秒内合并）；生成 Markdown 档案：标题、类型、键、属性表、最新指标表（含单位）、1 跳邻居列表（关系类型 + 标题，最多 50 个）、关联文档标题；以 `source_name = 'xb_node:<node_id>'` 写入超图（同源文档先删后写），并与节点建立 `EXACT_KEY` 实体链接。节点删除时删除档案文档 |
| 知识 → 节点 | 文档入库后，未链接到任何节点、`entity_type` 与某节点类型 `display_name` 或 `name` 相同、且出现次数 ≥ 3 的实体，写入候选列表 `GET /api/xblue/knowledge/node-candidates`；`POST /api/xblue/knowledge/node-candidates/{entity_id}/accept {type_name, node_key}` 创建节点并确认链接 |

AxQuery 召回因此可以直接回答“华东仓现在库存多少、服务哪些订单”（档案文档命中），并通过 Step 04 的 `related_nodes` 附带实时指标。

---

## 06.6 路由（`/api/xblue` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `POST /expr/validate`（`{type_name, expression}` → 问题列表与推断类型） | `STUDIO_EDIT` |
| `POST /expr/evaluate`（`{node_id, expression}` → 值与警告，不写库） | `DATA_READ` |
| `POST /aggregate/recompute` | `STUDIO_EDIT` |
| `GET /nodes/{id}/explain/{metric}` | `DATA_READ` |
| `GET /knowledge/node-candidates`、`POST /knowledge/node-candidates/{entity_id}/accept` | `STUDIO_EDIT` |

---

## 06.7 Step 06 验收

`scripts/gate/step06.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain expr rollup && cargo test -p axpoint-db mapping_exec aggregate_exec knowledge_sync)

# 映射物化：100 个订单节点
wait_job "$(api POST /api/xblue/mappings/$ORDER_MAPPING_ID/run '{"mode":"FULL"}' | jq -r .job_id)"
expect_eq "mapped-orders" "$(sql_app default "SELECT count(*) FROM xb_nodes WHERE type_name='Order' AND source_dataset_id='$DEMO_ORDERS_DATASET_ID'")" "100"
expect_eq "mapped-links"  "$(sql_app default "SELECT count(*) FROM xb_links l JOIN xb_nodes o ON o.id=l.dst_node_id WHERE l.link_type='FULFILLS' AND o.source_dataset_id='$DEMO_ORDERS_DATASET_ID'")" "100"

# 聚合与数据库对账：W-1 的 total_demand = 其 FULFILLS 关系指向的全部订单 demand 之和（独立 SQL 计算）
wait_aggregate_idle
EXP=$(sql_app default "SELECT sum((o.metrics->>'demand')::numeric) FROM xb_links l JOIN xb_nodes o ON o.id=l.dst_node_id WHERE l.src_node_id='$WAREHOUSE_NODE_ID' AND l.link_type='FULFILLS'" | awk '{printf "%.2f", $1}')
ACT=$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID | jq -r '.metrics.total_demand' | awk '{printf "%.2f", $1}')
expect_eq "agg-reconcile" "$ACT" "$EXP"

# 增量：源改 1 行金额 → 同步 → 映射增量 → 聚合自动刷新
sql_owner "UPDATE demo_src.orders SET amount = amount + 1000, updated_at = now() WHERE id = 2" >/dev/null
api PATCH /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID '{"sync_mode":"UPSERT","primary_key":["id"],"cursor_column":"updated_at"}' >/dev/null
wait_job "$(api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync | jq -r .job_id)"
wait_mapping_idle "$ORDER_MAPPING_ID"; wait_aggregate_idle
ACT2=$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID | jq -r '.metrics.total_demand' | awk '{printf "%.2f", $1}')
expect_eq "agg-incremental" "$(awk -v a="$ACT2" -v b="$ACT" 'BEGIN {printf "%.2f", a - b}')" "1000.00"
expect_json "incremental-mode" "$(api GET /api/xblue/mappings/$ORDER_MAPPING_ID/runs)" '.items[0].mode == "INCREMENTAL"'

# 派生指标与可解释性
expect_json "derived" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID/explain/fill_rate)" '.producer == "DERIVED" and (.inputs | length) == 2'

# 表达式校验
expect_json "expr-chain-cmp" "$(api POST /api/xblue/expr/validate '{"type_name":"Warehouse","expression":"1 < 2 < 3"}')" '(.issues | length) >= 1'

gate_pass step06
```

`wait_aggregate_idle`、`wait_mapping_idle` 由 `lib.sh` 提供（A5 §2）：轮询 `GET /api/axops/jobs?kind=GRAPH_AGGREGATE&status=PENDING,RUNNING`（或 `MAP_MATERIALIZE`）直到为空，超时 120 秒。

**通过标准**：最后一行 `GATE step06 PASS`。
