-- xBlueNode / AxPoint 动态超图与多维度向量存储 Schema
-- 适用数据库: PostgreSQL 16+ (需启用 pgvector 扩展)

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- 1. 文档节点表 (Doc)
CREATE TABLE IF NOT EXISTS hg_docs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    title       TEXT NOT NULL,
    source_name TEXT NOT NULL,
    summary     TEXT NOT NULL DEFAULT '',
    fts         TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('simple', coalesce(title, '') || ' ' || coalesce(summary, ''))
    ) STORED,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hg_docs_tenant ON hg_docs (tenant_id);
CREATE INDEX IF NOT EXISTS idx_hg_docs_fts ON hg_docs USING GIN (fts);

-- 2. 段落节点表 (Para)
CREATE TABLE IF NOT EXISTS hg_paras (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    doc_id          UUID NOT NULL REFERENCES hg_docs(id) ON DELETE CASCADE,
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    seq_no          INT NOT NULL,
    content         TEXT NOT NULL,
    entity_density  INT NOT NULL DEFAULT 0,
    fts             TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('simple', coalesce(content, ''))
    ) STORED,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hg_paras_doc ON hg_paras (doc_id, seq_no);
CREATE INDEX IF NOT EXISTS idx_hg_paras_tenant ON hg_paras (tenant_id);
CREATE INDEX IF NOT EXISTS idx_hg_paras_fts ON hg_paras USING GIN (fts);

-- 3. 实体节点表 (Entity)
CREATE TABLE IF NOT EXISTS hg_entities (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    name         TEXT NOT NULL,
    entity_type  TEXT NOT NULL DEFAULT 'CONCEPT',
    synonyms     TEXT[] NOT NULL DEFAULT '{}',
    occur_count  BIGINT NOT NULL DEFAULT 1,
    confidence   DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name, entity_type)
);

CREATE INDEX IF NOT EXISTS idx_hg_entities_tenant_name ON hg_entities (tenant_id, name);

-- 4. 超图关联边表 (Edges: CONTAINS / NEXT_PARA / MENTIONS / RELATES)
CREATE TABLE IF NOT EXISTS hg_edges (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    src_type     TEXT NOT NULL, -- 'DOC' | 'PARA' | 'ENTITY'
    src_id       UUID NOT NULL,
    dst_type     TEXT NOT NULL, -- 'DOC' | 'PARA' | 'ENTITY'
    dst_id       UUID NOT NULL,
    rel_type     TEXT NOT NULL, -- 'CONTAINS' | 'NEXT_PARA' | 'MENTIONS' | 'RELATES'
    fact         TEXT NOT NULL DEFAULT '',
    weight       DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    occur_count  BIGINT NOT NULL DEFAULT 1,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hg_edges_src ON hg_edges (tenant_id, src_id, rel_type);
CREATE INDEX IF NOT EXISTS idx_hg_edges_dst ON hg_edges (tenant_id, dst_id, rel_type);

-- 5. 多维度 Embedding 兼容表
-- 使用不锁死维度的 vector 字段，支持 384/768/1024/1536/3072 等任意维度无缝切换
CREATE TABLE IF NOT EXISTS hg_embeddings (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL DEFAULT 'default',
    node_type      TEXT NOT NULL, -- 'DOC' | 'PARA' | 'ENTITY'
    node_id        UUID NOT NULL,
    model_name     TEXT NOT NULL,
    embedding_dim  INT NOT NULL,
    embedding      vector NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (node_type, node_id, model_name)
);

CREATE INDEX IF NOT EXISTS idx_hg_emb_lookup ON hg_embeddings (tenant_id, node_type, embedding_dim);

-- 为常用维度建立条件部分 HNSW 索引（兼顾多维度兼容性与检索性能）
CREATE INDEX IF NOT EXISTS idx_hg_emb_hnsw_1024
    ON hg_embeddings USING hnsw ((embedding::vector(1024)) vector_cosine_ops)
    WHERE embedding_dim = 1024;

CREATE INDEX IF NOT EXISTS idx_hg_emb_hnsw_1536
    ON hg_embeddings USING hnsw ((embedding::vector(1536)) vector_cosine_ops)
    WHERE embedding_dim = 1536;

CREATE INDEX IF NOT EXISTS idx_hg_emb_hnsw_768
    ON hg_embeddings USING hnsw ((embedding::vector(768)) vector_cosine_ops)
    WHERE embedding_dim = 768;


-- ============================================================================
-- xBlueNode / AxPoint 节点建模、编排流水线与因果推演沙盘表
-- ============================================================================

-- 6. xBlueStudio 节点类型定义 (BlueNodeType)
CREATE TABLE IF NOT EXISTS xb_node_types (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    name         TEXT NOT NULL,
    display_name TEXT NOT NULL,
    properties   JSONB NOT NULL DEFAULT '[]'::jsonb,
    agg_rules    JSONB NOT NULL DEFAULT '[]'::jsonb,
    pos_x        DOUBLE PRECISION NOT NULL DEFAULT 120.0,
    pos_y        DOUBLE PRECISION NOT NULL DEFAULT 120.0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- 7. xBlueStudio 边类型定义 (BlueEdgeType)
CREATE TABLE IF NOT EXISTS xb_edge_types (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    src_type_name   TEXT NOT NULL,
    dst_type_name   TEXT NOT NULL,
    impact_weight   DOUBLE PRECISION NOT NULL DEFAULT 0.5,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name, src_type_name, dst_type_name)
);

-- 8. xBlueNode 业务节点实例 (BlueNode)
CREATE TABLE IF NOT EXISTS xb_nodes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    type_name   TEXT NOT NULL,
    node_key    TEXT NOT NULL,
    title       TEXT NOT NULL,
    metrics     JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, type_name, node_key)
);

-- 9. xBlueNode 业务关系实例 (BlueLink)
CREATE TABLE IF NOT EXISTS xb_links (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    link_type   TEXT NOT NULL,
    src_node_id UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    dst_node_id UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    weight      DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 10. AxFlow 智能编排流水线
CREATE TABLE IF NOT EXISTS ax_flows (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    steps       JSONB NOT NULL DEFAULT '[]'::jsonb,
    status      TEXT NOT NULL DEFAULT 'DRAFT',
    last_run_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 11. AxSim 因果推演沙盘
CREATE TABLE IF NOT EXISTS ax_sandboxes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    base_snapshot   JSONB NOT NULL DEFAULT '{}'::jsonb,
    actions         JSONB NOT NULL DEFAULT '[]'::jsonb,
    sim_result      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
