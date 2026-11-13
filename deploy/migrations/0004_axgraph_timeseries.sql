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
    value_text TEXT,
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
    m DATE;
    m_next DATE;
    ts_from TIMESTAMPTZ;
    ts_to TIMESTAMPTZ;
    created INT := 0;
    part TEXT;
BEGIN
    FOR m IN SELECT generate_series(date_trunc('month', now() AT TIME ZONE 'UTC')::date - make_interval(months => p_months_back),
                                    date_trunc('month', now() AT TIME ZONE 'UTC')::date + make_interval(months => p_months_ahead),
                                    interval '1 month')::date LOOP
        m_next := (m + interval '1 month')::date;
        ts_from := (m::text || ' 00:00:00+00')::timestamptz;
        ts_to := (m_next::text || ' 00:00:00+00')::timestamptz;
        part := format('xb_metric_series_%s', to_char(m, 'YYYYMM'));
        IF to_regclass(part) IS NULL THEN
            CREATE TEMP TABLE IF NOT EXISTS _ax_tmp_default_move (LIKE xb_metric_series) ON COMMIT DROP;
            TRUNCATE _ax_tmp_default_move;
            WITH moved AS (
                DELETE FROM xb_metric_series_default
                 WHERE ts >= ts_from AND ts < ts_to
                RETURNING *
            )
            INSERT INTO _ax_tmp_default_move SELECT * FROM moved;

            EXECUTE format('CREATE TABLE %I PARTITION OF xb_metric_series FOR VALUES FROM (%L) TO (%L)',
                           part, ts_from, ts_to);
            PERFORM ax_enable_tenant_rls(part::regclass);

            INSERT INTO xb_metric_series SELECT * FROM _ax_tmp_default_move;
            TRUNCATE _ax_tmp_default_move;
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
    cutoff := date_trunc('month', (now() AT TIME ZONE 'UTC') - make_interval(days => max_keep))::date;
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
    hll          BYTEA,
    freq         JSONB,
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
    algo        TEXT NOT NULL CHECK (algo IN (
        'PAGERANK','DEGREE','LABEL_PROPAGATION','CONNECTED_COMPONENTS','STRONGLY_CONNECTED','WEAKLY_CONNECTED'
    )),
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
