-- 0012_export_and_share.sql：出品任务、出品模板、分享链接（Step 12 §12.1）

CREATE TABLE IF NOT EXISTS xv_export_templates (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    export_format TEXT NOT NULL CHECK (export_format IN ('DOCX','PPTX')),
    file_path     TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    placeholders  TEXT[] NOT NULL DEFAULT '{}',
    created_by    UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS xv_export_jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    export_format TEXT NOT NULL CHECK (export_format IN ('XLSX','CSV','PDF','DOCX','PPTX','MARKDOWN')),
    source        JSONB NOT NULL,
    options       JSONB NOT NULL DEFAULT '{}'::jsonb,
    template_id   UUID REFERENCES xv_export_templates(id) ON DELETE SET NULL,
    job_id        UUID,
    status        TEXT NOT NULL DEFAULT 'PENDING'
                  CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','CANCELED','EXPIRED')),
    file_name     TEXT,
    file_path     TEXT,
    file_size     BIGINT,
    sha256        TEXT,
    error_message TEXT,
    created_by    UUID NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ,
    expires_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_xv_export_jobs_user ON xv_export_jobs (tenant_id, created_by, created_at DESC);

CREATE TABLE IF NOT EXISTS xv_share_links (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    token_prefix   TEXT NOT NULL,
    token_hash     TEXT NOT NULL UNIQUE,
    target_kind    TEXT NOT NULL CHECK (target_kind IN ('DASHBOARD','EXPORT_FILE','SCENARIO')),
    target_id      UUID NOT NULL,
    locked_filters JSONB NOT NULL DEFAULT '{}'::jsonb,
    allow_filters  BOOLEAN NOT NULL DEFAULT FALSE,
    allow_export   BOOLEAN NOT NULL DEFAULT FALSE,
    password_hash  TEXT,
    expires_at     TIMESTAMPTZ NOT NULL,
    max_views      INT CHECK (max_views IS NULL OR max_views > 0),
    view_count     INT NOT NULL DEFAULT 0,
    created_by     UUID NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at     TIMESTAMPTZ
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xv_export_templates','xv_export_jobs','xv_share_links'
]::regclass[]) AS t;

-- 分享页无登录态：以令牌哈希解析租户与目标（只返回有效链接）
CREATE OR REPLACE FUNCTION ax_resolve_share(p_token_hash TEXT)
RETURNS TABLE (share_id UUID, tenant_id TEXT, target_kind TEXT, target_id UUID, created_by UUID,
               locked_filters JSONB, allow_filters BOOLEAN, allow_export BOOLEAN, password_hash TEXT,
               max_views INT, view_count INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT s.id, s.tenant_id, s.target_kind, s.target_id, s.created_by, s.locked_filters,
           s.allow_filters, s.allow_export, s.password_hash, s.max_views, s.view_count
      FROM xv_share_links s
     WHERE s.token_hash = p_token_hash
       AND s.revoked_at IS NULL
       AND s.expires_at > NOW()
       AND (s.max_views IS NULL OR s.view_count < s.max_views);
$$;
REVOKE ALL ON FUNCTION ax_resolve_share(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_share(TEXT) TO axpoint_app;
