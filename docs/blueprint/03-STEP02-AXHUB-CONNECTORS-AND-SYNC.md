# 03 · Step 02：AxHub 连接器、同步与资产目录（02.1～02.9）

AxHub 把文件、关系库、国产库、分析型库、NoSQL、时序库、消息队列、REST / GraphQL 接口、Webhook 与日志统一接入为**数据集**（`ds_datasets`）。每个数据集在 DuckLake 中对应一张表，带快照版本、Schema 版本、同步任务历史、列级元数据、质量规则与血缘。

职责划分：

| 组件 | 职责 |
| :--- | :--- |
| Core（`axpoint-server` + `axpoint-db` + `axpoint-security`） | 连接与数据集的增删改查、凭证加密存储、同步任务入队与调度、Webhook 接收、资产目录检索、权限与审计 |
| Sidecar（`axpoint-parser/connectors`、`axpoint-parser/lake`） | 连接测试、对象发现、预览、按同步模式读取源数据并写入 DuckLake、质量校验、湖维护 |
| DuckLake | 目录在 PostgreSQL 模式 `lake_catalog`（角色 `axpoint_lake`），数据文件在 `${AXPOINT_DATA_ROOT}/lake/`（Parquet） |

凭证只在 Core 中解密，随单次 Sidecar 请求以内部网络传递，Sidecar 不落盘、不写日志。

---

## 02.0 目标、前置与契约

- **目标**：A3 中全部 32 种 `ConnectorType` 可创建、测试、发现、预览、同步；三种同步模式语义正确且可重试；Schema 变化可检测；文件上传一步入湖并同步进入知识库；Webhook 与消息流以微批方式持续入湖；资产目录可检索、可标注、可追溯血缘。
- **前置**：Step 01 通过。
- **契约清单**：`0003_data_fabric.sql`；`axpoint-domain::axhub`（类型）与 `ports::{ConnectionRepository, DatasetRepository}`；`schemas.py` 的 `ConnectorSpec / TestResult / DiscoveredObject / SyncRequest / SyncResult / CommitStagingRequest`；`docs/API_CONTRACT.md` AxHub 章节；`xbluenode-web/src/lib/api/axhub.ts`；A6 Step 02 依赖。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0003_data_fabric.sql` | §02.1 |
| 修改 | `deploy/docker-compose.test.yml` | 集成测试源：redpanda、mosquitto、minio、mongo、redis、elasticsearch、mysql、clickhouse、influxdb、atmoz/sftp（§02.1.3） |
| 新建 | `axpoint-core/crates/axpoint-domain/src/axhub.rs` | `Connection`、`Dataset`、`SyncJob`、`ColumnMeta`、`QualityRule`、`LineageEdge`、`SchemaChange` |
| 修改 | `axpoint-core/crates/axpoint-domain/src/ports.rs` | `ConnectionRepository`、`DatasetRepository`、`SidecarPort` |
| 新建 | `axpoint-core/crates/axpoint-security/src/keyring.rs` | AES-256-GCM 凭证密钥环（§02.2） |
| 新建 | `axpoint-core/crates/axpoint-db/src/axhub_repo.rs` | `PgStore` 对上述端口的实现 |
| 新建 | `axpoint-core/crates/axpoint-server/src/sidecar.rs` | `SidecarClient`（reqwest，内部令牌、租户与时区请求头、超时） |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/axhub.rs` | `/api/axhub/*`（§02.9.5） |
| 新建 | `axpoint-core/crates/axpoint-server/src/workers/sync_worker.rs` | `SYNC_DATASET`、`LAKE_MAINTENANCE`、`CREDENTIAL_ROTATE` 任务处理 |
| 新建 | `axpoint-core/crates/axpoint-server/src/workers/scheduler.rs` | 数据集定时同步（`cron` crate） |
| 新建 | `axpoint-parser/connectors/{__init__.py, base.py, registry.py, typemap.py, sql_family.py, domestic.py, analytic.py, nosql.py, timeseries.py, object_store.py, rest_api.py, graphql_api.py, streams.py, webhook.py}` | 连接器（§02.3～02.8，规格卡见 A3） |
| 新建 | `axpoint-parser/lake/{__init__.py, catalog.py, writer.py, evolve.py, quality.py, maintenance.py}` | DuckLake 写入、Schema 演进、质量校验、维护 |
| 修改 | `axpoint-parser/main.py` | `/connectors/*`、`/lake/*` 路由 |
| 新建 | `axpoint-parser/tests/{test_connectors_sql.py, test_connectors_nosql.py, test_connectors_ts.py, test_connectors_api.py, test_streams.py, test_lake_sync.py, test_evolve.py, test_quality.py}` | 测试 |
| 新建 | `xbluenode-web/src/pages/AxHubPage.tsx`、`src/components/axhub/{ConnectionForm.tsx, DatasetTable.tsx, SyncHistory.tsx, CatalogSearch.tsx, LineageGraph.tsx, UploadDropzone.tsx}`、`src/lib/api/axhub.ts` | 前端 |
| 修改 | `xbluenode-web/src/modules/registry.ts` | 注册 `axhub` |
| 新建 | `scripts/seed/02-axhub.sh` | 在演示库中建源表并通过 API 创建连接、数据集、上传样本文件 |
| 新建 | `scripts/gate/step02.sh` | 门禁 |

---

## 02.1 迁移与测试环境

### 02.1.1 `deploy/migrations/0003_data_fabric.sql`

```sql
-- 0003_data_fabric.sql：连接、凭证、数据集、列元数据、同步任务、上传、Webhook、流偏移、质量规则、术语、血缘

CREATE TABLE IF NOT EXISTS ds_credentials (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    key_version INT NOT NULL,
    nonce       BYTEA NOT NULL,                -- 12 字节
    ciphertext  BYTEA NOT NULL,                -- AES-256-GCM(JSON 秘密字段)，AAD = tenant_id || ':' || id
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rotated_at  TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS ds_connections (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             TEXT NOT NULL,
    name                  TEXT NOT NULL,
    connector_type        TEXT NOT NULL CHECK (connector_type IN (
        'FILE_UPLOAD','S3','SFTP',
        'POSTGRES','MYSQL','MSSQL','ORACLE','SQLITE','CLICKHOUSE','DAMENG','KINGBASE','OPENGAUSS','OCEANBASE',
        'TIDB','DORIS','STARROCKS','HIVE',
        'MONGODB','REDIS','ELASTICSEARCH','CASSANDRA','HBASE','NEO4J',
        'INFLUXDB','TDENGINE','IOTDB',
        'KAFKA','MQTT','PULSAR',
        'REST_API','GRAPHQL','WEBHOOK')),
    config                JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 只含非秘密字段（字段清单见 A3）
    credential_id         UUID REFERENCES ds_credentials(id),
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    status                TEXT NOT NULL DEFAULT 'UNTESTED' CHECK (status IN ('UNTESTED','OK','FAILED')),
    last_test_message     TEXT NOT NULL DEFAULT '',
    last_tested_at        TIMESTAMPTZ,
    created_by            UUID,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ds_datasets (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         TEXT NOT NULL,
    connection_id     UUID REFERENCES ds_connections(id) ON DELETE RESTRICT,
    name              TEXT NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    display_name      TEXT NOT NULL DEFAULT '',
    description       TEXT NOT NULL DEFAULT '',
    source_kind       TEXT NOT NULL CHECK (source_kind IN (
        'FILE_TABLE','SQL_TABLE','NOSQL_COLLECTION','TIMESERIES','STREAM','API_RESOURCE','WEBHOOK_STREAM','DERIVED_VIEW')),
    source_ref        JSONB NOT NULL DEFAULT '{}'::jsonb,       -- 表 / 集合 / 主题 / 端点 / 文件定位，结构见 A3
    sync_mode         TEXT NOT NULL DEFAULT 'SNAPSHOT' CHECK (sync_mode IN ('SNAPSHOT','APPEND','UPSERT')),
    primary_key       TEXT[] NOT NULL DEFAULT '{}',             -- UPSERT 必填
    cursor_column     TEXT,                                      -- APPEND / UPSERT 增量游标列
    cursor_value      TEXT,                                      -- 最近一次成功提交后的游标
    delete_column     TEXT,                                      -- UPSERT 软删除标记列（值为真表示删除）
    schema_policy     TEXT NOT NULL DEFAULT 'ADDITIVE' CHECK (schema_policy IN ('STRICT','ADDITIVE','EVOLVE')),
    lake_schema       TEXT NOT NULL,                             -- "tenant_<tenant_id>"
    lake_table        TEXT NOT NULL,                             -- 与 name 相同
    schema_json       JSONB NOT NULL DEFAULT '[]'::jsonb,        -- ColumnProfile[]
    schema_version    INT NOT NULL DEFAULT 0,
    row_count         BIGINT NOT NULL DEFAULT 0,
    last_snapshot_id  BIGINT,
    last_synced_at    TIMESTAMPTZ,
    schedule_cron     TEXT,                                      -- 5 段 cron，空表示只手动同步
    tags              TEXT[] NOT NULL DEFAULT '{}',
    owner_user_id     UUID,
    status            TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','READY','SYNCING','FAILED','DISABLED')),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name),
    CHECK (sync_mode <> 'UPSERT' OR cardinality(primary_key) > 0)
);
CREATE INDEX IF NOT EXISTS idx_ds_datasets_sched ON ds_datasets (tenant_id) WHERE schedule_cron IS NOT NULL;

CREATE TABLE IF NOT EXISTS ds_column_meta (
    tenant_id     TEXT NOT NULL,
    dataset_id    UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    column_name   TEXT NOT NULL,
    data_type     TEXT NOT NULL CHECK (data_type IN (
        'STRING','INT64','FLOAT64','DECIMAL','BOOLEAN','DATE','TIMESTAMP','BINARY','JSON','LIST','STRUCT')),
    pii_category  TEXT NOT NULL DEFAULT 'NONE' CHECK (pii_category IN (
        'NONE','PHONE','ID_CARD','EMAIL','BANK_CARD','NAME','ADDRESS','SECRET')),
    pii_confirmed BOOLEAN NOT NULL DEFAULT FALSE,   -- 人工确认后不再被自动识别覆盖
    description   TEXT NOT NULL DEFAULT '',
    business_term TEXT NOT NULL DEFAULT '',
    unit          TEXT NOT NULL DEFAULT '',
    ordinal       INT NOT NULL,
    PRIMARY KEY (dataset_id, column_name)
);

CREATE TABLE IF NOT EXISTS ds_sync_jobs (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    dataset_id     UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    job_id         UUID NOT NULL,                  -- ax_jobs.id
    batch_id       UUID NOT NULL,                  -- 写入湖表的 _ax_batch_id，重试复用
    sync_mode      TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'PENDING'
                   CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','CANCELED','EXPIRED')),
    cursor_from    TEXT,
    cursor_to      TEXT,
    rows_read      BIGINT NOT NULL DEFAULT 0,
    rows_written   BIGINT NOT NULL DEFAULT 0,
    rows_deleted   BIGINT NOT NULL DEFAULT 0,
    snapshot_id    BIGINT,
    schema_change  JSONB NOT NULL DEFAULT '{}'::jsonb,   -- SchemaChange
    quality_result JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_code     TEXT,
    error_message  TEXT,
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ds_sync_jobs_ds ON ds_sync_jobs (tenant_id, dataset_id, created_at DESC);

CREATE TABLE IF NOT EXISTS ds_uploads (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    filename    TEXT NOT NULL,
    sha256      TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    parse_id    UUID,
    doc_id      UUID,                               -- 文本轨写入的 hg_docs.id
    dataset_ids UUID[] NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','PARSED','COMMITTED','FAILED')),
    error_code  TEXT,
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ds_uploads_sha ON ds_uploads (tenant_id, sha256);

CREATE TABLE IF NOT EXISTS ds_webhooks (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         TEXT NOT NULL,
    dataset_id        UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    slug              TEXT NOT NULL CHECK (slug ~ '^[a-z0-9-]{3,64}$'),
    auth_scheme       TEXT NOT NULL DEFAULT 'HMAC_SHA256' CHECK (auth_scheme IN ('HMAC_SHA256','BEARER')),
    credential_id     UUID NOT NULL REFERENCES ds_credentials(id),
    record_path       TEXT NOT NULL DEFAULT '$',     -- JSONPath，定位请求体中的记录数组
    max_body_bytes    INT NOT NULL DEFAULT 1048576,
    enabled           BOOLEAN NOT NULL DEFAULT TRUE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug)
);

CREATE TABLE IF NOT EXISTS ds_webhook_buffer (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    webhook_id  UUID NOT NULL REFERENCES ds_webhooks(id) ON DELETE CASCADE,
    delivery_id TEXT NOT NULL,                       -- 请求头 X-AxPoint-Delivery 或请求体 SHA-256
    payload     JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    flushed_at  TIMESTAMPTZ,
    UNIQUE (webhook_id, delivery_id)
);
CREATE INDEX IF NOT EXISTS idx_ds_webhook_buffer_pending ON ds_webhook_buffer (webhook_id, id) WHERE flushed_at IS NULL;

CREATE TABLE IF NOT EXISTS ds_stream_offsets (
    tenant_id   TEXT NOT NULL,
    dataset_id  UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    partition   TEXT NOT NULL,                       -- Kafka: "<topic>:<partition>"；Pulsar: 订阅名；MQTT: 主题过滤器
    last_offset TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (dataset_id, partition)
);

CREATE TABLE IF NOT EXISTS ds_quality_rules (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    dataset_id  UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    rule_kind   TEXT NOT NULL CHECK (rule_kind IN ('NOT_NULL','UNIQUE','RANGE','REGEX','ALLOWED_VALUES','FRESHNESS','ROW_COUNT_MIN')),
    column_name TEXT,
    params      JSONB NOT NULL DEFAULT '{}'::jsonb,
    on_fail     TEXT NOT NULL DEFAULT 'WARN' CHECK (on_fail IN ('WARN','BLOCK')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS ds_glossary_terms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    term        TEXT NOT NULL,
    definition  TEXT NOT NULL DEFAULT '',
    synonyms    TEXT[] NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, term)
);

-- 血缘：数据源 → 数据集 → 指标 / 节点类型 / 流程 / 看板（后续 Step 写入各自的边）
CREATE TABLE IF NOT EXISTS ds_lineage (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  TEXT NOT NULL,
    src_kind   TEXT NOT NULL CHECK (src_kind IN ('CONNECTION','UPLOAD','DATASET','METRIC','NODE_TYPE','FLOW','AGENT','DASHBOARD')),
    src_id     TEXT NOT NULL,
    dst_kind   TEXT NOT NULL CHECK (dst_kind IN ('DATASET','METRIC','NODE_TYPE','FLOW','AGENT','DASHBOARD','EXPORT','DOC')),
    dst_id     TEXT NOT NULL,
    via        TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, src_kind, src_id, dst_kind, dst_id)
);

-- 资产目录全文检索
ALTER TABLE ds_datasets ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(name,'') || ' ' || coalesce(display_name,'') || ' ' || coalesce(description,'')
                || ' ' || array_to_string(tags, ' '))) STORED;
CREATE INDEX IF NOT EXISTS idx_ds_datasets_fts ON ds_datasets USING GIN (fts);

-- 文本轨与上传记录关联
ALTER TABLE hg_docs ADD COLUMN IF NOT EXISTS upload_id UUID;

-- 同一数据集同一时刻只允许一个待执行或执行中的同步任务
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_jobs_sync_dataset ON ax_jobs ((payload->>'dataset_id'))
    WHERE kind = 'SYNC_DATASET' AND status IN ('PENDING','RUNNING');

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ds_credentials','ds_connections','ds_datasets','ds_column_meta','ds_sync_jobs','ds_uploads','ds_webhooks',
    'ds_webhook_buffer','ds_stream_offsets','ds_quality_rules','ds_glossary_terms','ds_lineage'
]::regclass[]) AS t;

-- Webhook 接收入口在鉴权前需要按 (tenant, slug) 定位配置
CREATE OR REPLACE FUNCTION ax_resolve_webhook(p_tenant TEXT, p_slug TEXT)
RETURNS TABLE (webhook_id UUID, dataset_id UUID, auth_scheme TEXT, credential_id UUID, record_path TEXT, max_body_bytes INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT w.id, w.dataset_id, w.auth_scheme, w.credential_id, w.record_path, w.max_body_bytes
      FROM ds_webhooks w
     WHERE w.tenant_id = p_tenant AND w.slug = p_slug AND w.enabled;
$$;
REVOKE ALL ON FUNCTION ax_resolve_webhook(TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_webhook(TEXT, TEXT) TO axpoint_app;

-- 定时调度扫描全部租户的数据集
CREATE OR REPLACE FUNCTION ax_scheduled_datasets()
RETURNS TABLE (tenant_id TEXT, dataset_id UUID, schedule_cron TEXT, last_synced_at TIMESTAMPTZ)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.id, d.schedule_cron, d.last_synced_at
      FROM ds_datasets d
     WHERE d.schedule_cron IS NOT NULL AND d.status IN ('READY','FAILED');
$$;
REVOKE ALL ON FUNCTION ax_scheduled_datasets() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_scheduled_datasets() TO axpoint_app;
```

### 02.1.2 DuckLake 目录初始化

`axpoint-parser/lake/catalog.py` 是唯一建立湖连接的位置：

```python
from __future__ import annotations

import duckdb
from settings import settings


def _sql_quote(val: str) -> str:
    return "'" + val.replace("'", "''") + "'"


def lake_connection() -> duckdb.DuckDBPyConnection:
    con = duckdb.connect(config={"allow_unsigned_extensions": "false", "autoinstall_known_extensions": "false"})
    con.execute("LOAD ducklake; LOAD postgres;")      # 扩展在镜像构建阶段预装（见 01 §00.9）
    root = settings.axpoint_data_root.resolve()
    lake_dir = f"{root}/lake"
    tenants_dir = f"{root}/tenants"
    dsn = f"ducklake:postgres:{settings.lake_catalog_libpq()}"
    con.execute(
        f"ATTACH {_sql_quote(dsn)} AS lake (DATA_PATH {_sql_quote(lake_dir + '/')}, METADATA_SCHEMA 'lake_catalog')"
    )
    con.execute(
        f"SET allowed_directories = [{_sql_quote(lake_dir)}, {_sql_quote(tenants_dir)}]; "
        "SET threads = 4; SET memory_limit = '2GB'; "
        "SET enable_external_access = false; SET lock_configuration = true;"
    )
    return con


def tenant_schema(tenant_id: str) -> str:
    return f'"tenant_{tenant_id}"'                   # tenant_id 已按 ^[a-z0-9_-]{1,64}$ 校验
```

- `settings.lake_catalog_libpq()`（01 §00.7）把 `AXPOINT_LAKE_CATALOG_DSN` 转为 libpq 关键字串（`dbname=... host=... port=... user=axpoint_lake password=...`）。
- `ATTACH` 语句不支持预处理参数占位符，所有连接串与目录均经 `_sql_quote` 转义；`allowed_directories` 限定为 `<data_root>/lake`（DuckLake 数据文件）与 `<data_root>/tenants`（暂存 Parquet），随后执行 `enable_external_access = false` 与 `lock_configuration = true`，阻断对其他任何路径或网络的访问（Core 侧只读会话采用同样顺序，见 Step 04 §04.2）。
- 每张湖表含系统列：`_ax_batch_id UUID`、`_ax_synced_at TIMESTAMP`；流与 Webhook 数据集额外含 `_ax_msg_id VARCHAR`。
- 每个租户首次写入时执行 `CREATE SCHEMA IF NOT EXISTS lake."tenant_<id>"`。

### 02.1.3 `deploy/docker-compose.test.yml`

| 服务 | 镜像 | 端口 | 用途 |
| :--- | :--- | :--- | :--- |
| `redpanda` | `redpandadata/redpanda`（固定 24.2.x 发布标签） | 19092 | Kafka 协议 |
| `mosquitto` | `eclipse-mosquitto:2` | 11883 | MQTT（配置 `allow_anonymous true` 只用于测试） |
| `pulsar` | `apachepulsar/pulsar:3.3`（命令 `bin/pulsar standalone`） | 16650 | Pulsar |
| `minio` | `minio/minio`（固定一个 RELEASE 标签） | 19000 | S3 |
| `sftp` | `atmoz/sftp:alpine` | 12222 | SFTP |
| `mongo` | `mongo:8` | 27018 | MongoDB |
| `redis` | `redis:8` | 16379 | Redis |
| `elasticsearch` | `docker.elastic.co/elasticsearch/elasticsearch`（9.x，`xpack.security.enabled=false`） | 19200 | Elasticsearch |
| `mysql` | `mysql:8.4` | 13306 | MySQL 协议族（同时用于 TiDB / Doris / StarRocks / OceanBase-MySQL 模式的协议级测试） |
| `clickhouse` | `clickhouse/clickhouse-server:25` | 18123 | ClickHouse |
| `influxdb` | `influxdb:3-core` | 18181 | InfluxDB 3（2.x 协议路径由 `heavy` 测试用 `influxdb:2.7` 覆盖） |

达梦、人大金仓、openGauss、TDengine（`tdengine/tdengine:3`）、IoTDB（`apache/iotdb:2`）、Oracle、SQL Server、Hive、Cassandra、HBase、Neo4j 的集成测试使用 `testcontainers` 在测试函数内按需启动（镜像清单见 [A3](./A3-CONNECTOR-SPEC-CARDS.md) 各卡片“测试”栏），标记 `@pytest.mark.heavy`，CI 默认 `-m "not heavy"`，nightly 工作流执行 `-m heavy`。

```bash
docker compose -f deploy/docker-compose.test.yml up -d --wait
```

---

## 02.2 凭证密钥环（`axpoint-security::keyring`）

```rust
/// AES-256-GCM 凭证密钥环。密钥来自 AXPOINT_CREDENTIAL_KEYS="1:<b64>,2:<b64>"，版本号最大者为当前加密密钥。
pub struct Keyring {
    keys: std::collections::BTreeMap<u32, aes_gcm::Key<aes_gcm::Aes256Gcm>>,
}

/// 密文三元组。
pub struct Sealed {
    pub key_version: u32,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

impl Keyring {
    /// 解析环境变量；任一密钥长度不是 32 字节、版本重复或为空时返回 `AppError::Config`。
    pub fn from_env_value(raw: &str) -> Result<Self, AppError>;
    /// 用当前密钥加密；AAD 固定为 "{tenant_id}:{credential_id}"，防止密文在租户或记录间挪用。
    pub fn seal(&self, tenant_id: &str, credential_id: uuid::Uuid, plaintext: &[u8]) -> Result<Sealed, AppError>;
    /// 按 key_version 选择密钥解密；版本不存在返回 `AppError::Config("CREDENTIAL_KEY_MISSING")`。
    pub fn open(&self, tenant_id: &str, credential_id: uuid::Uuid, sealed: &Sealed) -> Result<zeroize::Zeroizing<Vec<u8>>, AppError>;
    /// 当前密钥版本。
    pub fn current_version(&self) -> u32;
}
```

规则：

1. Nonce 每次用 `OsRng` 生成 12 字节随机数。
2. 明文是秘密字段的 JSON（例 `{"password":"..."}`），字段清单由 A3 每张卡片的“秘密字段”栏定义。
3. API 写入：请求体中秘密字段与普通字段混合提交，Core 按 `ConnectorSpec.secret_fields` 拆分，普通字段写 `ds_connections.config`，秘密字段加密写 `ds_credentials`。
4. API 读取：秘密字段一律不返回，响应中以 `"secret_fields_set": ["password"]` 表明已设置。`PATCH` 时省略的秘密字段保持不变，显式传 `null` 表示清除。
5. 轮换：`POST /api/axhub/credentials/rotate`（`DATA_ADMIN`）入队 `CREDENTIAL_ROTATE`，任务逐条解密并以当前版本重新加密，写 `rotated_at`；全部完成后才允许从环境变量中移除早期版本密钥。
6. `AXPOINT_CREDENTIAL_KEYS` 未设置时，Core 启动在 `production` 环境下失败，在 `development` / `test` 环境下用随机密钥启动并打印 `WARN credential keyring is ephemeral`。

**测试**（`axpoint-security/tests/keyring.rs`）：往返加解密；篡改密文 1 字节失败；AAD 中租户不同失败；用版本 1 加密、密钥环含 1 与 2 时可解密且 `current_version()==2`；从环境变量解析非法 base64 返回错误。

---

## 02.3 Sidecar 连接器框架

### 02.3.1 协议（`connectors/base.py`）

```python
from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from typing import Any, Literal, Protocol

import pyarrow as pa


@dataclass(frozen=True)
class ConnectorSpec:
    connector_type: str
    display_name: str
    category: Literal["FILE", "RELATIONAL", "DOMESTIC", "ANALYTIC", "NOSQL", "TIMESERIES", "STREAM", "API"]
    config_schema: dict[str, Any]          # JSON Schema，前端据此渲染表单
    secret_fields: tuple[str, ...]
    supports_modes: tuple[str, ...]        # SNAPSHOT / APPEND / UPSERT 子集
    supports_discover: bool
    image_tier: Literal["lite", "full"]    # 驱动所在镜像


@dataclass
class DiscoveredObject:
    ref: dict[str, Any]                    # 写入 ds_datasets.source_ref
    display_name: str
    source_kind: str
    columns: list[dict[str, Any]]          # name / native_type / data_type / nullable
    primary_key: list[str]
    suggested_cursor: str | None
    estimated_rows: int | None


@dataclass
class ReadBatch:
    batch: pa.RecordBatch
    cursor_to: str | None                  # 本批最大游标（字符串化）
    offsets: dict[str, str]                # 流类：partition → offset


class Connector(Protocol):
    spec: ConnectorSpec

    def test(self) -> tuple[bool, str]: ...
    def discover(self) -> list[DiscoveredObject]: ...
    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table: ...
    def read(self, ref: dict[str, Any], mode: str, cursor_column: str | None,
             cursor_from: str | None, batch_rows: int) -> Iterator[ReadBatch]: ...
    def close(self) -> None: ...
```

`registry.py`：`REGISTRY: dict[str, type[Connector]]` 以 `connector_type` 为键；`build(connector_type, config, secrets, allow_private_network) -> Connector`。驱动未安装（lite 镜像中的 full 驱动）时 `build` 返回 `415 UNSUPPORTED_FORMAT`，`message` 为 `driver not available in lite image`。

### 02.3.2 网络出站约束

- 所有带主机名的连接器在 `build` 时调用 `security.check_outbound_url()`（01.1），`allow_private_network=false` 时拒绝私网地址；本地开发连接 compose 内部服务时由 `DATA_ADMIN` 显式开启。
- REST / GraphQL 连接器使用自定义 `httpx.HTTPTransport`，每一跳重定向重新校验。
- 连接超时 10 秒，单次读取请求超时 60 秒（可在 `config.timeout_s` 调整，上限 600）。

### 02.3.3 类型映射（`connectors/typemap.py`）

每个连接器把原生类型映射为 `DataType`，未知类型一律 `STRING`。公共映射：

| 原生类型族 | `DataType` |
| :--- | :--- |
| `smallint/int/integer/bigint/tinyint/mediumint/serial/int2/int4/int8/NUMBER(p,0) p≤18` | `INT64` |
| `numeric/decimal/NUMBER(p,s) s>0/money` | `DECIMAL`（精度 > 38 时 `STRING`） |
| `real/float/double/float4/float8/BINARY_DOUBLE` | `FLOAT64` |
| `bool/boolean/bit(1)` | `BOOLEAN` |
| `date` | `DATE` |
| `timestamp/timestamptz/datetime/datetime2/DATETIMEOFFSET/TIMESTAMP WITH TIME ZONE` | `TIMESTAMP`（统一转 UTC） |
| `char/varchar/text/nvarchar/clob/uuid/enum/inet` | `STRING` |
| `bytea/blob/varbinary/raw` | `BINARY` |
| `json/jsonb/MongoDB 文档/ES object` | `JSON` |
| `array/ARRAY<T>/MongoDB 数组` | `LIST` |
| ClickHouse `Tuple`、Hive `struct` | `STRUCT` |

各连接器的特有映射（例 Oracle `NUMBER` 无精度、MSSQL `uniqueidentifier`、TDengine `NCHAR`）列在 A3 对应卡片中，并有表驱动测试 `tests/test_typemap.py`。

### 02.3.4 Sidecar 接口

| 方法与路径 | 请求体 | 响应 |
| :--- | :--- | :--- |
| `GET /connectors/types` | — | `ConnectorSpec[]`（含当前镜像是否可用 `available: bool`） |
| `POST /connectors/test` | `{connector_type, config, secrets, allow_private_network}` | `{ok, message, latency_ms}` |
| `POST /connectors/discover` | 同上 | `DiscoveredObject[]` |
| `POST /connectors/preview` | 同上 + `{ref, limit≤1000}` | `{columns: ColumnProfile[], rows: object[]}`（PII 列脱敏） |
| `POST /connectors/sync` | `SyncRequest`（§02.9.1） | `SyncResult` |
| `POST /lake/commit_staging` | `CommitStagingRequest`（§02.6） | `SyncResult[]` |
| `POST /lake/maintenance` | `{tenant_id, keep_days}` | `{merged_files, expired_snapshots, deleted_files}` |
| `POST /lake/preview` | `{tenant_id, lake_table, limit, snapshot_id?}` | `{columns, rows}` |
| `GET /lake/snapshots` | `?tenant_id&lake_table` | `[{snapshot_id, committed_at, rows_added, rows_deleted}]` |

---

## 02.4 关系型、国产与分析型数据库连接器

| 文件 | 连接器 | 读取方式 |
| :--- | :--- | :--- |
| `sql_family.py` | `POSTGRES`、`MYSQL`、`MSSQL`、`ORACLE`、`SQLITE` | 发现：SQLAlchemy `inspect()`（表、视图、列、主键、注释、`pg_class.reltuples` / `information_schema.TABLES.TABLE_ROWS` 估算行数）。读取：`connectorx.read_sql(..., return_type="arrow", partition_on=<整数主键>, partition_num=4)`；SQLite 用标准库 `sqlite3`，文件路径必须位于 `${AXPOINT_DATA_ROOT}/tenants/<tenant>/files/` 下 |
| `domestic.py` | `DAMENG`（`dmPython`）、`KINGBASE`（PostgreSQL 协议，`psycopg`）、`OPENGAUSS`（PostgreSQL 协议，`psycopg`；服务端须允许 MD5 或 SCRAM 认证，`sha256` 专有认证需在服务端 `password_encryption_type` 设为 1）、`OCEANBASE`（MySQL 租户模式，`pymysql`）、`TIDB`（MySQL 协议） | 发现：方言系统视图（达梦 `ALL_TABLES / ALL_TAB_COLUMNS / ALL_CONSTRAINTS`；其余同 PG / MySQL 族）。读取：服务端游标分批 `fetchmany(batch_rows)` 转 Arrow |
| `analytic.py` | `CLICKHOUSE`（`clickhouse-connect`，`query_arrow_stream`）、`DORIS` / `STARROCKS`（MySQL 协议 `pymysql`；可选 Arrow Flight SQL 端口 `config.flight_port`）、`HIVE`（`pyhive`，HiveServer2，`config.auth` 取 `NONE/LDAP/KERBEROS`） | 同上 |

**读取 SQL 构造**（全部连接器共用 `sql_family.build_select`）：

```text
SNAPSHOT : SELECT <cols> FROM <schema>.<table>
APPEND   : SELECT <cols> FROM <schema>.<table> WHERE <cursor> > :cursor_from ORDER BY <cursor>
UPSERT   : SELECT <cols> FROM <schema>.<table> WHERE <cursor> >= :cursor_from ORDER BY <cursor>
           （使用 >= 并依赖主键合并去重，避免同一时间戳在两批之间被漏读）
```

- 标识符经 A1 §8.2 白名单校验并按方言加引号（PG 族 `"x"`、MySQL 族 `` `x` ``、MSSQL `[x]`、Oracle / 达梦 `"X"`）。
- 游标值以参数绑定，类型按游标列 `DataType` 还原（`TIMESTAMP` 用 ISO 8601，`INT64` 用整数）。
- 自定义 SQL 数据集（`source_ref.query`）只允许单条 `SELECT`，由 `sqlglot.parse_one(sql, read=<方言>)` 校验根节点为 `Select` 且无 DML/DDL 子节点，增量时包装为 `SELECT * FROM (<query>) q WHERE ...`。
- 主键缺失时：`discover` 返回空 `primary_key`，数据集创建时若选择 `UPSERT` 返回 400 `INVALID_ARGUMENT`。

**测试**（`tests/test_connectors_sql.py`）：compose 中的 PostgreSQL（使用主库的 `demo_src` 模式）、MySQL、ClickHouse 各执行 test / discover / preview / SNAPSHOT / APPEND / UPSERT；`heavy` 标记下覆盖其余库。断言：发现结果含主键与游标建议；三种模式写入行数与源一致（§02.9.2 场景表）。

---

## 02.5 NoSQL 与时序库连接器

| 连接器 | 发现 | 读取与增量 | 扁平化 |
| :--- | :--- | :--- | :--- |
| `MONGODB` | `list_collection_names()`；对每个集合 `$sample` 1 000 条推断字段 | 游标列默认 `_id`（ObjectId 单调）或指定时间字段；`find({cursor: {$gt: v}}).sort(cursor).batch_size(n)` | 嵌套文档按 01.9 规则扁平化，深度 6；数组按 `json_explode` 规则 |
| `REDIS` | `SCAN` 前缀统计（`config.key_patterns`），按键类型分组 | 只支持 `SNAPSHOT`：`HASH` → 每键一行（字段为列）；`STRING` → `key,value`；`ZSET` → `key,member,score`；`STREAM` → 按条目 ID 增量 `XRANGE`（`APPEND`） | — |
| `ELASTICSEARCH` | `GET _mapping` | `search_after` + PIT 分页；增量用时间字段 `range` 查询 | `object` 扁平化，`nested` 拆子表 |
| `CASSANDRA` | `system_schema.tables/columns` | 只支持 `SNAPSHOT`（全表分页 `fetch_size`），或按 `config.time_bucket_column` 做 `APPEND` | `map/set/list` → `JSON` / `LIST` |
| `HBASE` | Thrift `getTableNames` + 列族 | `scan` 行键范围；`config.column_mapping` 指定 `family:qualifier → 列名 + DataType` | — |
| `NEO4J` | `CALL db.labels()`、`CALL db.relationshipTypes()`、`apoc.meta.schema`（无 APOC 时抽样） | 节点：`MATCH (n:<Label>) WHERE n.<cursor> > $v RETURN n`；关系：`MATCH ()-[r:<TYPE>]->() RETURN startNode(r).<key>, endNode(r).<key>, r` | 节点属性为列；关系数据集含 `src_key`、`dst_key` |
| `INFLUXDB` | `buckets()`、`schema.measurements()`、`schema.fieldKeys()`（2.x 版本）；3.x 版本用 SQL `information_schema` | Flux / SQL `range(start: cursor_from)` 按 `_time` 增量（`APPEND`） | 输出长表：`ts, measurement, tags(JSON), field, value` 或按 `config.pivot=true` 输出宽表 |
| `TDENGINE` | `SHOW STABLES`、`DESCRIBE` | `SELECT * FROM <stable> WHERE ts > ?`，子表名作为 `tbname` 列 | 标签列直接成列 |
| `IOTDB` | `SHOW TIMESERIES` | `SELECT * FROM root.<path> WHERE time > ? ALIGN BY DEVICE` | `device` 列 + 各测点列 |

时序数据集（`source_kind=TIMESERIES`）在 Step 03 可直接映射为 `xb_metric_series`（指标路径 = 节点类型 + 节点键 + 指标名）。

**测试**（`tests/test_connectors_nosql.py`、`tests/test_connectors_ts.py`）：Mongo 嵌套文档样本扁平化列名与快照一致；Redis `HASH` 100 键得到 100 行；ES `nested` 拆出子表；InfluxDB 写入 1 000 点后 APPEND 两次，第二次只读到两次同步之间写入的点（使用写入批次标识断言）。

---

## 02.6 文件入湖（上传、S3、SFTP）

### 02.6.1 上传接口 `POST /api/axhub/uploads`

`multipart/form-data`：`file`（必填）、`to_knowledge`（`true` 默认，文本轨写入知识库）、`to_datasets`（`true` 默认，表格轨写入数据集）、`dataset_prefix`（可选，数据集名前缀，默认取文件名规范化结果）、Step 01 的 `log_format / json_explode / ocr`。

处理序列（全部在一个 `PARSE_FILE` 任务中执行，接口立即返回 `202 {upload_id, job_id}`）：

1. Core 流式落盘到 `${AXPOINT_DATA_ROOT}/tenants/<tenant>/uploads/<upload_id>/<sha256>`，计算 SHA-256；同租户同 SHA-256 且状态 `COMMITTED` 的上传已存在时，直接返回已有结果并标注 `deduplicated: true`。
2. Core 调用 Sidecar `POST /parse/unified`。
3. `to_knowledge=true`：Core 把 `markdown` 交给现有入库管线（`/api/documents/ingest` 的内部函数），`hg_docs.upload_id` 记录来源，写 `ds_lineage(UPLOAD → DOC)`。
4. `to_datasets=true`：Core 为每张 `ExtractedTable` 创建或复用数据集（名称 `<prefix>_<table_name>`，`source_kind=FILE_TABLE`、`sync_mode=SNAPSHOT`），然后调用 Sidecar `POST /lake/commit_staging`：

```python
class CommitStagingRequest(BaseModel):
    tenant_id: str
    parse_id: str
    items: list[CommitItem]

class CommitItem(BaseModel):
    staging_path: str           # 相对 AXPOINT_DATA_ROOT，必须位于 tenants/<tenant_id>/staging/<parse_id>/ 下
    dataset_id: str
    lake_table: str
    batch_id: str
    schema_policy: Literal["STRICT", "ADDITIVE", "EVOLVE"]
    expected_schema: list[ColumnProfile]   # 已有数据集的当前 Schema；新数据集为空
```

5. Sidecar 以 SNAPSHOT 语义提交（§02.9.2），返回 `snapshot_id`、`schema_change`；Core 写 `ds_sync_jobs`、`ds_column_meta`（`pii_confirmed=false` 的列以本次画像覆盖）、`ds_lineage(UPLOAD → DATASET)`，`ds_uploads.status=COMMITTED`，删除暂存目录，写审计 `FILE_UPLOAD` 与外发事件 `dataset.synced`。

### 02.6.2 S3 / SFTP

- `S3`（`s3fs`，配置 `endpoint_url, bucket, prefix, region, path_style`，秘密 `access_key_id, secret_access_key`）与 `SFTP`（`paramiko`，配置 `host, port, base_dir`，秘密 `password` 或 `private_key`）。
- `discover` 列出 `prefix` 下匹配 `config.include_glob`（默认 `**/*.{csv,tsv,txt,xlsx,xls,json,jsonl,xml,log,parquet}`）的文件，每个文件或每个 `config.group_pattern` 分组（例 `orders_*.csv` 合并为一个数据集）对应一个对象。
- `read`：`SNAPSHOT` 读取全部匹配文件；`APPEND` 只读修改时间晚于 `cursor_from` 的文件（游标为 ISO 8601 修改时间 + 文件路径，按 `(mtime, path)` 排序）。文件内容下载到暂存目录后走 Step 01 解析器，`parquet` 直接读取。

**测试**：minio 中放入 3 个 `orders_*.csv`，`group_pattern` 合并为一个数据集，行数等于三者之和；再放入第 4 个文件后 APPEND 只读入该文件的行。

---

## 02.7 API、GraphQL、Webhook 与日志采集

### 02.7.1 `REST_API`（`connectors/rest_api.py`，`httpx` 实现）

`config` 结构：

```json
{
  "base_url": "https://erp.example.com/api",
  "auth": {"type": "OAUTH2_CLIENT_CREDENTIALS", "token_url": "https://erp.example.com/oauth/token", "scope": "read"},
  "default_headers": {"Accept": "application/json"},
  "rate_limit_rps": 5,
  "timeout_s": 60,
  "resources": [
    {
      "name": "orders",
      "path": "/v1/orders",
      "method": "GET",
      "params": {"status": "all"},
      "record_path": "$.data.items[*]",
      "primary_key": ["id"],
      "pagination": {"type": "CURSOR", "cursor_path": "$.data.next_cursor", "cursor_param": "cursor", "page_size_param": "limit", "page_size": 200},
      "incremental": {"param": "updated_since", "cursor_field": "updated_at", "format": "ISO8601"}
    }
  ]
}
```

- `auth.type`：`NONE`、`BASIC`（秘密 `username,password`）、`BEARER`（秘密 `token`）、`API_KEY_HEADER`（`header_name`，秘密 `api_key`）、`API_KEY_QUERY`（`param_name`，秘密 `api_key`）、`OAUTH2_CLIENT_CREDENTIALS`（秘密 `client_id,client_secret`，令牌缓存到过期前 60 秒）。
- `pagination.type`：`NONE`、`PAGE_NUMBER`（`page_param, start, page_size_param, page_size`，空页停止）、`OFFSET`（`offset_param, limit_param, limit, total_path?`）、`CURSOR`（`cursor_path, cursor_param`，游标为空停止）、`LINK_HEADER`（RFC 8288 `rel="next"`）。
- 重试：`tenacity`，对 429 / 5xx / 连接错误指数退避（0.5 秒起、最多 5 次），遵守 `Retry-After`。
- 记录用 `jsonpath-ng` 按 `record_path` 提取，按 01.9 规则扁平化；每个资源一个数据集（`source_kind=API_RESOURCE`）。
- `discover`：对每个资源请求第一页，推断列。
- Schema 漂移：同步时把推断的 JSON Schema 与上次存档比对（01.9 的 `genson` 结果），差异写入 `schema_change`。

### 02.7.2 `GRAPHQL`（`connectors/graphql_api.py`）

`config`：`endpoint, auth（同上）, resources[{name, query, variables, record_path, pagination: {type: "RELAY", page_info_path, after_variable}}]`。`discover` 执行内省查询列出 Query 根字段；`query` 必须是 `query` 操作（解析 AST 拒绝 `mutation` / `subscription`）。

### 02.7.3 `WEBHOOK`

- 创建：`POST /api/axhub/webhooks {dataset_name, slug, auth_scheme, record_path}` → 创建 `WEBHOOK_STREAM` 数据集（`sync_mode=APPEND`）与 `ds_webhooks`，生成 32 字节随机秘密，响应中只返回一次：`{url, secret}`。
- 接收：`POST /api/axhub/webhooks/{tenant_id}/{slug}/ingest`（不经过 `AuthPrincipal`，A1 §2 规则 2 的外部入口）：
  1. `ax_resolve_webhook(tenant_id, slug)` 定位配置；不存在返回 404。
  2. 请求体超过 `max_body_bytes` 返回 413。
  3. `HMAC_SHA256`：校验请求头 `X-AxPoint-Signature: sha256=<hex(HMAC(secret, timestamp + "." + body))>` 与 `X-AxPoint-Timestamp`（与服务器时间相差 ≤ 300 秒），常量时间比较；`BEARER`：校验 `Authorization: Bearer <secret>`。失败返回 401 `SIGNATURE_INVALID`。
  4. 以租户上下文写 `ds_webhook_buffer`（`delivery_id` 取 `X-AxPoint-Delivery` 或请求体 SHA-256，重复投递命中唯一约束后直接返回 200）。
  5. 返回 `202 {accepted: true}`。
  6. 每个 Webhook 数据集默认 `schedule_cron='* * * * *'`，同步任务把未刷入的缓冲行按 `record_path` 展开后 APPEND 入湖，`_ax_msg_id = delivery_id || ':' || 序号`，成功后写 `flushed_at`。缓冲行按保留策略 `MESSAGE` 清理。
- 限流：`tower_governor` 按 `(tenant_id, slug)` 每秒 50 次、突发 100。

### 02.7.4 日志持续采集

日志文件的一次性导入走 01.11 + 02.6。持续采集不在本系统内运行采集进程，采用以下两种标准接法之一（`docs/blueprint` 只规定配置，示例配置文件放在 `deploy/collectors/`）：

| 接法 | 配置文件 | 说明 |
| :--- | :--- | :--- |
| Vector → Webhook | `deploy/collectors/vector.toml` | `sources.app_logs`（`file`，`include = ["/var/log/app/*.log"]`，多行规则与 01.11 相同）→ `transforms.parse`（`remap`，解析为 01.11 输出列）→ `sinks.axpoint`（`http`，`uri = "https://<host>/api/axhub/webhooks/<tenant>/<slug>/ingest"`，`encoding.codec = "json"`，`batch.max_events = 500`，`auth.strategy = "bearer"`，`auth.token = "${AXPOINT_WEBHOOK_SECRET}"`） |
| Fluent Bit → Kafka | `deploy/collectors/fluent-bit.conf` | `[INPUT] tail` + `[FILTER] multiline` → `[OUTPUT] kafka`（`topics app-logs`），系统侧以 `KAFKA` 连接器消费（§02.8） |

**测试**（`tests/test_connectors_api.py`）：`respx` 模拟 REST 服务的五种分页方式与 OAuth2 令牌端点，断言记录数与请求次数；429 重试后成功；GraphQL `mutation` 被拒绝。Webhook 由门禁脚本端到端验证（§02.10）。

---

## 02.8 消息流（Kafka、MQTT、Pulsar）

所有消息连接器采用**微批拉取**：每次 `SYNC_DATASET` 任务在 `config.max_wait_s`（默认 20）内最多读取 `config.max_messages`（默认 10 000）条，入湖后提交位点。数据集默认 `schedule_cron='* * * * *'`，可改为更长间隔。投递语义为“至少一次 + 湖表按 `_ax_msg_id` 去重”，整体效果为精确一次入湖。

| 连接器 | 驱动 | 位点与 `_ax_msg_id` | 关键配置 |
| :--- | :--- | :--- | :--- |
| `KAFKA` | `confluent-kafka` | `group.id = axpoint-<tenant>-<dataset_id>`，`enable.auto.commit=false`；`_ax_msg_id = topic:partition:offset`；入湖成功后 `commit(asynchronous=false)` 并写 `ds_stream_offsets` | `bootstrap_servers, topics, security_protocol(PLAINTEXT/SSL/SASL_SSL), sasl_mechanism(PLAIN/SCRAM-SHA-256/SCRAM-SHA-512), value_format(JSON/AVRO/CSV/RAW), schema_registry_url, start(EARLIEST/LATEST)`；秘密 `sasl_username, sasl_password, ssl_key_pem` |
| `MQTT` | `paho-mqtt` 2.x | 持久会话（`clean_start=false`、`client_id = axpoint-<dataset_id>`、QoS 1），代理在两次拉取间为会话保留消息；`_ax_msg_id = topic:<payload SHA-256>:<接收时间毫秒>`，`ds_stream_offsets` 记录最后接收时间 | `host, port, tls, topic_filters, qos(0/1), value_format`；秘密 `username, password` |
| `PULSAR` | `pulsar-client` | `Shared` 订阅 `axpoint-<dataset_id>`；入湖后逐条 `acknowledge`；`_ax_msg_id = message_id` 序列化字符串 | `service_url, topics, subscription_initial_position, value_format`；秘密 `token` |

- 解码：`JSON` 按 01.9 扁平化；`AVRO` 通过 Schema Registry（`confluent_kafka.schema_registry.avro.AvroDeserializer`）；`CSV` 每条消息一行；`RAW` 输出 `key, value(BINARY), headers(JSON)`。
- 入湖：`INSERT INTO t SELECT * FROM batch WHERE _ax_msg_id NOT IN (SELECT _ax_msg_id FROM t WHERE _ax_synced_at > now() - INTERVAL 1 DAY)`，与位点提交同属一次同步任务；位点提交失败时下次重读的消息被去重过滤。
- 实时指标：流数据集可在 Step 06 的映射中直接写入 `xb_metric_series`，延迟上限为一次拉取间隔加映射任务耗时。

**测试**（`tests/test_streams.py`）：redpanda 写入 1 000 条 JSON，连续两次同步合计入湖 1 000 行；人为让第一次同步在提交位点前抛异常，重跑后仍为 1 000 行（去重生效）；mosquitto 在两次同步之间发布 50 条 QoS 1 消息，第二次同步读到 50 条。

---

## 02.9 同步语义、调度、Schema 演进、质量、目录与前端

### 02.9.1 同步执行链路

```text
POST /api/axhub/datasets/{id}/sync  或  调度器  或  AxFlow SOURCE_INGEST 算子
  → Core: INSERT ax_jobs(kind=SYNC_DATASET, payload={dataset_id}, idempotency_key=<dataset_id>:<请求时间>)
          ON CONFLICT 命中 uq_ax_jobs_sync_dataset 时返回已有 job_id（200 而非 202）
  → sync_worker 通过 ax_lease_jobs 领取任务
      → 在租户事务中读取数据集、连接、解密凭证，写 ds_sync_jobs(RUNNING, batch_id)
      → 调用 Sidecar POST /connectors/sync（超时 = 任务租约 - 30 秒，租约默认 3600 秒，执行中每 60 秒续租）
      → 成功：在一个事务内写 ds_sync_jobs(SUCCEEDED, 统计)、ds_datasets(cursor_value, schema_json, schema_version,
              row_count, last_snapshot_id, last_synced_at, status=READY)、ds_column_meta、ds_stream_offsets、
              ax_outbox('dataset.synced', {dataset_id, snapshot_id, rows_written})、sec_audit_logs(DATA_SYNC)
      → 失败：ds_sync_jobs(FAILED, error_code, error_message)；可重试错误（UPSTREAM_*）按 ax_jobs.attempt 退避重试，
              不可重试错误（SCHEMA_BREAKING_CHANGE、QUALITY_BLOCKED、凭证错误）直接 FAILED 并写 ds_datasets.status=FAILED
```

```python
class SyncRequest(BaseModel):
    tenant_id: str
    dataset_id: str
    batch_id: str                 # 重试复用同一 batch_id
    connector_type: str
    config: dict[str, Any]
    secrets: dict[str, str]
    allow_private_network: bool
    source_ref: dict[str, Any]
    sync_mode: Literal["SNAPSHOT", "APPEND", "UPSERT"]
    primary_key: list[str]
    cursor_column: str | None
    cursor_from: str | None
    delete_column: str | None
    stream_offsets: dict[str, str]
    lake_table: str
    schema_policy: Literal["STRICT", "ADDITIVE", "EVOLVE"]
    expected_schema: list[ColumnProfile]
    quality_rules: list[QualityRule]
    batch_rows: int = 50_000


class SyncResult(BaseModel):
    dataset_id: str
    rows_read: int
    rows_written: int
    rows_deleted: int
    cursor_to: str | None
    stream_offsets: dict[str, str]
    snapshot_id: int
    schema: list[ColumnProfile]
    schema_change: SchemaChange
    quality: QualityReport
```

### 02.9.2 三种同步模式在 DuckLake 中的实现（`lake/writer.py`）

所有写入在**一个 DuckLake 事务**中完成（`BEGIN ... COMMIT`），提交即产生一个新快照；失败回滚，湖表保持上一快照。

| 模式 | 实现 | 重试幂等 |
| :--- | :--- | :--- |
| `SNAPSHOT` | 读取全部批次写入临时表 `tmp_<batch>`，然后 `BEGIN; DELETE FROM lake.s.t; INSERT INTO lake.s.t SELECT * FROM tmp_<batch>; COMMIT;`（表不存在时 `CREATE TABLE lake.s.t AS SELECT ...`） | 本身幂等 |
| `APPEND` | `BEGIN; DELETE FROM lake.s.t WHERE _ax_batch_id = :batch; INSERT INTO lake.s.t SELECT * FROM tmp_<batch>; COMMIT;` | 先删本批次再插入，重试不会重复 |
| `UPSERT` | `BEGIN; MERGE INTO lake.s.t AS d USING tmp_<batch> AS s ON <pk 等值> WHEN MATCHED AND s.<delete_column> THEN DELETE WHEN MATCHED THEN UPDATE SET * WHEN NOT MATCHED AND NOT coalesce(s.<delete_column>, false) THEN INSERT *; COMMIT;`；运行时 DuckLake 版本不支持 `MERGE INTO` 时，以同一事务内 `DELETE FROM d USING s WHERE <pk>` + `INSERT` 实现 | 按主键合并，重试结果相同 |

游标：Sidecar 返回 `cursor_to`（本次读取的最大游标值），Core 只在 Sidecar 返回成功（湖事务已提交）之后写入 `ds_datasets.cursor_value`。Core 写库失败而湖已提交时，下次以上一次游标重读：`APPEND` 产生新的 `batch_id` 会重复插入，因此 `APPEND` 在 Sidecar 侧对 `cursor_column` 再做一次过滤 `WHERE cursor > (SELECT max(cursor) FROM lake.s.t)`，保证不重复。

源端删除：`UPSERT` 依赖 `delete_column`；无软删除标记的源，可为数据集配置间隔较长的 `SNAPSHOT` 调度做全量对齐（在同一数据集上切换模式时 `schema_version` 不变）。

**验收场景**（`tests/test_lake_sync.py`，源表 `demo_src.orders(id PK, amount, status, updated_at)`）：

| # | 操作 | 期望湖表状态 |
| :--- | :--- | :--- |
| 1 | 源 100 行，`SNAPSHOT` | 100 行，快照 +1 |
| 2 | 源删除 10 行，`SNAPSHOT` | 90 行 |
| 3 | 切换 `APPEND`（游标 `updated_at`），源插入 20 行 | 110 行 |
| 4 | 以同一 `batch_id` 重放第 3 步 | 110 行 |
| 5 | 切换 `UPSERT`（主键 `id`），源修改 5 行金额、插入 5 行 | 115 行，5 行金额为新值 |
| 6 | 源把 3 行 `is_deleted` 置真，`UPSERT`（`delete_column=is_deleted`） | 112 行 |
| 7 | `GET /lake/snapshots` | 至少 6 个快照，可用第 1 个快照 `AT (VERSION => id)` 读到 100 行 |

### 02.9.3 Schema 演进（`lake/evolve.py`）

```python
class SchemaChange(BaseModel):
    added: list[ColumnProfile] = []
    removed: list[str] = []
    widened: list[tuple[str, str, str]] = []   # (列, 原类型, 新类型)
    narrowed: list[tuple[str, str, str]] = []
    breaking: bool = False
```

- 允许的拓宽：`INT64 → DECIMAL → FLOAT64`、`DATE → TIMESTAMP`、任意类型 → `STRING`。其余类型变化为收窄。
- 策略：

| `schema_policy` | 增加列 | 删除列 | 拓宽 | 收窄 |
| :--- | :--- | :--- | :--- | :--- |
| `STRICT` | 失败 | 失败 | 失败 | 失败 |
| `ADDITIVE`（默认） | `ALTER TABLE ADD COLUMN` | 失败 | `ALTER TABLE ALTER COLUMN TYPE` | 失败 |
| `EVOLVE` | 同上 | 保留列，写入空值 | 同上 | 列改为 `STRING` |

- 失败时 `error_code=SCHEMA_BREAKING_CHANGE`，`schema_change` 写明差异；前端在同步历史中展示差异并提供“改用 EVOLVE 策略后重试”。
- 每次 Schema 变化 `schema_version + 1`，下游（指标、节点映射）在 Step 04 / 06 中按 `schema_version` 校验引用列。

### 02.9.4 质量校验（`lake/quality.py`，`pandera`）

在湖事务提交前对 `tmp_<batch>` 执行 `ds_quality_rules`：

| `rule_kind` | `params` | 判定 |
| :--- | :--- | :--- |
| `NOT_NULL` | — | 空值数 = 0 |
| `UNIQUE` | — | 去重数 = 行数（`UPSERT` 模式下对主键自动启用） |
| `RANGE` | `{min, max}` | 全部值在闭区间 |
| `REGEX` | `{pattern}` | 全部非空值匹配 |
| `ALLOWED_VALUES` | `{values: []}` | 全部非空值属于集合 |
| `FRESHNESS` | `{max_lag_minutes}` | `max(列) ≥ now - lag` |
| `ROW_COUNT_MIN` | `{min}` | 行数 ≥ min |

`on_fail=BLOCK` 的规则失败时回滚湖事务，任务失败 `error_code=QUALITY_BLOCKED`；`WARN` 只写 `quality_result`。Step 08 的规则引擎可订阅 `dataset.synced` 事件中的质量结果触发告警。

### 02.9.5 Core 接口 `/api/axhub/*`

| 方法与路径 | 权限 | 说明 |
| :--- | :--- | :--- |
| `GET /connector-types` | `DATA_READ` | 代理 Sidecar `/connectors/types` |
| `GET /connections`、`POST /connections` | `DATA_READ` / `DATA_ADMIN` | 创建时 `allow_private_network=true` 需要 `DATA_ADMIN` |
| `GET /connections/{id}`、`PATCH /connections/{id}`、`DELETE /connections/{id}` | `DATA_READ` / `DATA_ADMIN` | 删除时存在数据集返回 409 |
| `POST /connections/{id}/test` | `DATA_ADMIN` | 写 `status`、`last_test_message` |
| `GET /connections/{id}/discover` | `DATA_ADMIN` | 返回 `DiscoveredObject[]` |
| `POST /connections/{id}/preview` | `DATA_READ` | `{ref, limit}` |
| `POST /credentials/rotate` | `DATA_ADMIN` | 入队 `CREDENTIAL_ROTATE` |
| `GET /datasets`、`POST /datasets` | `DATA_READ` / `DATA_SYNC` | 列表支持 `?q=&tag=&source_kind=` |
| `GET /datasets/{id}`、`PATCH /datasets/{id}`、`DELETE /datasets/{id}` | `DATA_READ` / `DATA_SYNC` / `DATA_ADMIN` | 删除时同时 `DROP TABLE` 湖表（由 Sidecar 执行） |
| `POST /datasets/{id}/sync` | `DATA_SYNC` | `202 {job_id}` 或 `200 {job_id, existing: true}` |
| `GET /datasets/{id}/sync-jobs` | `DATA_READ` | 分页 |
| `GET /datasets/{id}/preview?limit=&snapshot_id=` | `DATA_READ` | 按 Step 13 列策略脱敏 |
| `GET /datasets/{id}/snapshots` | `DATA_READ` | 快照列表 |
| `GET /datasets/{id}/columns`、`PUT /datasets/{id}/columns/{column}` | `DATA_READ` / `DATA_ADMIN` | 列描述、业务术语、单位、PII 确认 |
| `GET /datasets/{id}/quality-rules`、`POST /datasets/{id}/quality-rules`、`DELETE /quality-rules/{rule_id}` | `DATA_READ` / `DATA_ADMIN` | |
| `GET /datasets/{id}/lineage?depth=3` | `DATA_READ` | 上下游边 |
| `POST /uploads`、`GET /uploads/{id}` | `DATA_SYNC` / `DATA_READ` | §02.6 |
| `POST /webhooks`、`GET /webhooks`、`DELETE /webhooks/{id}` | `DATA_ADMIN` | §02.7.3 |
| `POST /webhooks/{tenant_id}/{slug}/ingest` | Webhook 秘密 | 外部入口 |
| `GET /catalog/search?q=` | `DATA_READ` | `ds_datasets.fts` + `ds_column_meta`（列名、描述、业务术语）+ `ds_glossary_terms` 同义词扩展 |
| `GET /glossary`、`POST /glossary`、`DELETE /glossary/{id}` | `DATA_READ` / `DATA_ADMIN` | 术语表 |

### 02.9.6 调度器（`workers/scheduler.rs`）

- 每 30 秒调用 `ax_scheduled_datasets()`，对每个数据集把 5 段表达式前补秒字段 `0 ` 后用 `cron::Schedule::from_str` 解析，以租户时区计算“上次同步之后的下一次触发时间”，已到期则入队（唯一索引保证不重复）。
- 多副本部署时调度器只在持有 `pg_try_advisory_lock(hashtext('ax_scheduler'))` 的实例上运行。
- `LAKE_MAINTENANCE` 任务每个租户每日 03:00（租户时区）入队一次，调用 Sidecar `/lake/maintenance`，内部依次执行 `ducklake_merge_adjacent_files`、`ducklake_expire_snapshots(older_than => now() - INTERVAL '<keep_days> days')`（`keep_days` 取保留策略 `LAKE_SNAPSHOT`）、`ducklake_cleanup_old_files`。

### 02.9.7 前端 `AxHubPage`

| 区域 | 内容 |
| :--- | :--- |
| 连接 | 按 `category` 分组的连接器卡片；`ConnectionForm` 依据 `config_schema` 渲染（秘密字段为密码框，编辑时显示“已设置”）；测试按钮显示延迟与错误 |
| 发现与建数据集 | 发现结果表格勾选 → 选择同步模式（不支持的模式置灰）、主键、游标列、调度 cron、Schema 策略 |
| 数据集 | `DatasetTable`：名称、来源、模式、行数、最近同步、状态；详情抽屉含预览、列元数据编辑、快照、同步历史（`SyncHistory` 展示 Schema 差异与质量结果）、质量规则、血缘（`LineageGraph`，ECharts graph） |
| 上传 | `UploadDropzone`：多文件拖放，显示解析警告、生成的数据集与知识文档链接 |
| 目录 | `CatalogSearch`：全文检索数据集与列，术语表维护 |

Playwright 冒烟：`e2e/axhub.spec.ts` 打开 `/hub`，看到演示连接 `demo_pg` 状态 `OK`，打开数据集 `demo_orders` 预览出现表格。

---

## 02.10 验收

`scripts/seed/02-axhub.sh` 以 `sql_owner` 在主库建 `demo_src.orders(id INT PRIMARY KEY, amount NUMERIC(12,2), status TEXT, customer_name TEXT, phone TEXT, warehouse_code TEXT, is_deleted BOOLEAN DEFAULT FALSE, updated_at TIMESTAMPTZ)` 并写入 100 行（`amount = id * 10.5`；`status` 按 `id % 3` 取 `PAID` / `PENDING` / `CANCELED`；`phone` 为校验合法的 11 位手机号；`warehouse_code` 按 `id % 2 = 0` 取 `W-1`，否则 `W-2`；`updated_at` 为固定基准时间加 `id` 分钟），授予 `axpoint_app` 只读，然后通过 API：创建连接 `demo_pg`（`POSTGRES`，`allow_private_network=true`）、发现并创建数据集 `demo_orders`（`SNAPSHOT`）、上传 `supply_chain.xlsx`、创建 Webhook `demo-events`；输出 `DEMO_PG_CONN_ID`、`DEMO_ORDERS_DATASET_ID`、`SUPPLY_UPLOAD_ID`、`WEBHOOK_SLUG`、`WEBHOOK_SECRET` 到 `.gate/ids.env`。

`scripts/gate/step02.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-security keyring && cargo test -p axpoint-db axhub)
(cd axpoint-parser && pytest -q -m "not heavy" tests/test_connectors_sql.py tests/test_connectors_nosql.py \
   tests/test_connectors_ts.py tests/test_connectors_api.py tests/test_streams.py tests/test_lake_sync.py \
   tests/test_evolve.py tests/test_quality.py tests/test_typemap.py)

# 连接与秘密不回显
expect_json "conn-ok"        "$(api POST /api/axhub/connections/$DEMO_PG_CONN_ID/test)" '.ok == true'
expect_json "secret-hidden"  "$(api GET /api/axhub/connections/$DEMO_PG_CONN_ID)" '(.config | has("password") | not) and (.secret_fields_set | index("password") != null)'
expect_eq   "cipher-at-rest" "$(sql_owner "SELECT count(*) FROM ds_credentials WHERE convert_from(ciphertext,'LATIN1') LIKE '%axpoint%'")" "0"

# 同步与幂等
JOB=$(api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync | jq -r .job_id)
wait_job "$JOB"
expect_json "orders-rows"    "$(api GET /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID)" '.row_count == 100 and .status == "READY"'
expect_json "single-flight"  "$( { api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync; api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync; } | jq -s . )" '.[0].job_id == .[1].job_id'
wait_dataset_idle "$DEMO_ORDERS_DATASET_ID"

# 上传入湖 + 入知识库
expect_json "upload-commit"  "$(api GET /api/axhub/uploads/$SUPPLY_UPLOAD_ID)" '.status == "COMMITTED" and (.dataset_ids | length) == 4 and .doc_id != null'

# Webhook：签名正确 202，错误 401，重复投递不重复入库
BODY='[{"device":"D-1","temp":21.5},{"device":"D-2","temp":22.0}]'
TS=$(date +%s); SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" -hex | awk '{print $2}')
hook() { curl -s -o /dev/null -w '%{http_code}' -X POST "$CORE_BASE_URL/api/axhub/webhooks/default/$WEBHOOK_SLUG/ingest" \
  -H "Content-Type: application/json" -H "X-AxPoint-Timestamp: $TS" -H "X-AxPoint-Delivery: d-1" -H "X-AxPoint-Signature: sha256=$1" -d "$BODY"; }
expect_eq "webhook-ok"   "$(hook "$SIG")" "202"
expect_eq "webhook-dup"  "$(hook "$SIG")" "200"
expect_eq "webhook-bad"  "$(hook deadbeef)" "401"
WH_DS=$(api GET "/api/axhub/datasets?q=$WEBHOOK_SLUG" | jq -r '.items[0].id')
wait_job "$(api POST /api/axhub/datasets/$WH_DS/sync | jq -r .job_id)"
expect_json "webhook-rows" "$(api GET /api/axhub/datasets/$WH_DS)" '.row_count == 2'

# 租户隔离与目录
expect_eq   "rls-ds"         "$(sql_app other "SELECT count(*) FROM ds_datasets")" "0"
expect_json "catalog"        "$(api GET '/api/axhub/catalog/search?q=orders')" '[.items[].name] | index("demo_orders") != null'

gate_pass step02
```

`wait_job`、`wait_dataset_idle` 由 `lib.sh` 提供（A5 §2）：轮询 `GET /api/axops/jobs/{id}`（Step 00 提供的任务查询接口），超时 120 秒判失败。

**通过标准**：输出最后一行 `GATE step02 PASS`；nightly 工作流中 `pytest -m heavy tests/test_connectors_*.py` 全部通过（A3 中每张卡片的集成测试）。
