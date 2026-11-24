-- 0020_saved_charts.sql：独立可复用可视化图表资产表与审计动作扩展

CREATE TABLE IF NOT EXISTS xv_charts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    name            TEXT NOT NULL CHECK (char_length(trim(name)) BETWEEN 1 AND 128),
    description     TEXT NOT NULL DEFAULT '',
    widget_kind     TEXT NOT NULL,
    data_source     JSONB NOT NULL DEFAULT '{}'::jsonb,
    encoding        JSONB NOT NULL DEFAULT '{}'::jsonb,
    options         JSONB NOT NULL DEFAULT '{}'::jsonb,
    owner_dept      TEXT NOT NULL DEFAULT 'default',
    sensitivity     TEXT NOT NULL DEFAULT 'INTERNAL'
                    CHECK (sensitivity IN ('PUBLIC', 'INTERNAL', 'CONFIDENTIAL', 'SECRET')),
    shared          BOOLEAN NOT NULL DEFAULT TRUE,
    created_by      UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE INDEX IF NOT EXISTS idx_xv_charts_tenant_updated
    ON xv_charts (tenant_id, updated_at DESC);

SELECT ax_enable_tenant_rls('xv_charts');

ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE','STUDIO_SNAPSHOT_RESTORE','IM_ACCOUNT_BIND',
        'CHART_PUBLISH'
    ));
