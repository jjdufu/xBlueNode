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
    rule_kind   TEXT NOT NULL CHECK (rule_kind IN (
        'NOT_NULL','UNIQUE','RANGE','REGEX','ALLOWED_VALUES','ENUM_SET','FRESHNESS','FRESHNESS_MINUTES','ROW_COUNT_MIN')),
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

-- 资产目录全文检索（PostgreSQL 16 要求 GENERATED STORED 表达式完全 IMMUTABLE）
CREATE OR REPLACE FUNCTION ax_text_array_join(p_arr TEXT[])
RETURNS TEXT LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT coalesce(array_to_string(p_arr, ' '), '');
$$;

ALTER TABLE ds_datasets ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(name,'') || ' ' || coalesce(display_name,'') || ' ' || coalesce(description,'')
                || ' ' || ax_text_array_join(tags))) STORED;
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
