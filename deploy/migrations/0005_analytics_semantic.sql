-- 0005_analytics_semantic.sql：指标语义层、保存查询、查询历史、问数会话、变换草案

CREATE TABLE IF NOT EXISTS an_metrics (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    name           TEXT NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    display_name   TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    dataset_id     UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE RESTRICT,
    expression     TEXT NOT NULL,
    filter_sql     TEXT NOT NULL DEFAULT '',
    time_column    TEXT,
    default_grain  TEXT NOT NULL DEFAULT 'DAY' CHECK (default_grain IN ('MINUTE','HOUR','DAY','MONTH')),
    dimensions     JSONB NOT NULL DEFAULT '[]'::jsonb,
    unit           TEXT NOT NULL DEFAULT '',
    format         TEXT NOT NULL DEFAULT '',
    synonyms       TEXT[] NOT NULL DEFAULT '{}',
    certified      BOOLEAN NOT NULL DEFAULT FALSE,
    schema_version INT NOT NULL,
    owner_user_id  UUID,
    version        INT NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);
ALTER TABLE an_metrics ADD COLUMN IF NOT EXISTS fts TSVECTOR GENERATED ALWAYS AS (
    to_tsvector('simple', coalesce(name,'') || ' ' || coalesce(display_name,'') || ' ' || coalesce(description,'')
                || ' ' || ax_text_array_join(synonyms))) STORED;
CREATE INDEX IF NOT EXISTS idx_an_metrics_fts ON an_metrics USING GIN (fts);

CREATE TABLE IF NOT EXISTS an_saved_queries (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('SQL','PIVOT','METRIC','ASK')),
    spec          JSONB NOT NULL,
    parameters    JSONB NOT NULL DEFAULT '[]'::jsonb,
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
    linked_objects JSONB NOT NULL DEFAULT '{}'::jsonb,
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
    preview             JSONB NOT NULL DEFAULT '{}'::jsonb,
    target_dataset_name TEXT NOT NULL CHECK (target_dataset_name ~ '^[a-z][a-z0-9_]{0,62}$'),
    status              TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','APPLIED','DISCARDED')),
    dataset_id          UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'an_metrics','an_saved_queries','an_query_history','an_ask_turns','an_transform_drafts'
]::regclass[]) AS t;

GRANT USAGE, SELECT ON SEQUENCE an_query_history_id_seq TO axpoint_app;
