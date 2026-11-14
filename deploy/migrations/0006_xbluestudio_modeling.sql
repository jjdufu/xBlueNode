-- 0006_xbluestudio_modeling.sql: multi-subgraph spaces, type metadata extensions, derived rules,
-- schema drafts, schema versions, dataset-to-graph mappings, and mapping execution runs.

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
ALTER TABLE xb_edge_types ADD COLUMN IF NOT EXISTS propagation   JSONB NOT NULL DEFAULT '{}'::jsonb;
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
    schema_doc     JSONB NOT NULL,
    diff           JSONB NOT NULL DEFAULT '{}'::jsonb,
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
    base_version  INT NOT NULL,
    schema_doc    JSONB NOT NULL,
    ai_context    JSONB NOT NULL DEFAULT '{}'::jsonb,
    issues        JSONB NOT NULL DEFAULT '[]'::jsonb,
    status        TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PUBLISHED','DISCARDED')),
    created_by    UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_xb_schema_drafts_subgraph
    ON xb_schema_drafts (tenant_id, subgraph_slug, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS xb_mappings (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        TEXT NOT NULL,
    subgraph_slug    TEXT NOT NULL DEFAULT 'main',
    name             TEXT NOT NULL,
    dataset_id       UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE RESTRICT,
    node_type        TEXT NOT NULL,
    spec             JSONB NOT NULL,
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    schema_version   INT NOT NULL,
    last_snapshot_id BIGINT,
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
    reject_samples JSONB NOT NULL DEFAULT '[]'::jsonb,
    status         TEXT NOT NULL CHECK (status IN ('RUNNING','SUCCEEDED','FAILED')),
    error_message  TEXT,
    started_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_xb_mapping_runs_mapping
    ON xb_mapping_runs (tenant_id, mapping_id, started_at DESC);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xb_subgraphs','xb_schema_heads','xb_schema_versions','xb_schema_drafts','xb_mappings','xb_mapping_runs'
]::regclass[]) AS t;

GRANT SELECT, INSERT, UPDATE, DELETE ON
    xb_subgraphs, xb_schema_heads, xb_schema_versions, xb_schema_drafts, xb_mappings, xb_mapping_runs
TO axpoint_app;

-- Default enterprise main subgraph (slug='main') for every existing tenant
INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
SELECT tenant_id, 'main', '企业主图', TRUE FROM sec_tenants ON CONFLICT DO NOTHING;

-- Baseline version 1 snapshot for tenants that already have main-graph node types
INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
SELECT DISTINCT tenant_id, 'main', 1 FROM xb_node_types WHERE subgraph_slug = 'main' ON CONFLICT DO NOTHING;

INSERT INTO xb_schema_versions (tenant_id, subgraph_slug, version, schema_doc, change_summary)
SELECT t.tenant_id, 'main', 1,
       jsonb_build_object(
         'node_types', COALESCE((SELECT jsonb_agg(jsonb_build_object(
              'name', n.name,
              'display_name', n.display_name,
              'description', n.description,
              'icon', n.icon,
              'color', n.color,
              'title_template', n.title_template,
              'properties', n.properties,
              'agg_rules', n.agg_rules,
              'derived_rules', n.derived_rules,
              'sync_to_knowledge', n.sync_to_knowledge,
              'pos_x', n.pos_x,
              'pos_y', n.pos_y))
              FROM xb_node_types n WHERE n.tenant_id = t.tenant_id AND n.subgraph_slug = 'main'), '[]'::jsonb),
         'edge_types', COALESCE((SELECT jsonb_agg(jsonb_build_object(
              'name', e.name,
              'src_type_name', e.src_type_name,
              'dst_type_name', e.dst_type_name,
              'impact_weight', e.impact_weight,
              'cardinality', e.cardinality))
              FROM xb_edge_types e WHERE e.tenant_id = t.tenant_id AND e.subgraph_slug = 'main'), '[]'::jsonb),
         'mappings', '[]'::jsonb),
       'baseline'
  FROM (SELECT DISTINCT tenant_id FROM xb_node_types WHERE subgraph_slug = 'main') t
ON CONFLICT DO NOTHING;
