-- 0021_actions_and_edge_dynamics.sql：一等公民对象动作契约、关系边动态指标、边时序表与子图运行时认证记录

ALTER TABLE xb_links
    ADD COLUMN IF NOT EXISTS properties JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS metrics JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS row_version BIGINT NOT NULL DEFAULT 1;

ALTER TABLE xb_node_types
    ADD COLUMN IF NOT EXISTS actions JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE xb_edge_types
    ADD COLUMN IF NOT EXISTS spec JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE IF NOT EXISTS xb_link_metric_series (
    tenant_id       TEXT NOT NULL,
    link_id         UUID NOT NULL REFERENCES xb_links(id) ON DELETE CASCADE,
    metric_key      TEXT NOT NULL,
    ts              TIMESTAMPTZ NOT NULL,
    val_num         DOUBLE PRECISION,
    val_text        TEXT,
    source_tag      TEXT NOT NULL DEFAULT 'INGEST',
    PRIMARY KEY (tenant_id, link_id, metric_key, ts)
);

CREATE INDEX IF NOT EXISTS idx_xb_link_metric_series_lookup
    ON xb_link_metric_series (tenant_id, link_id, metric_key, ts DESC);

SELECT ax_enable_tenant_rls('xb_link_metric_series');

CREATE TABLE IF NOT EXISTS xb_runtime_certifications (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           TEXT NOT NULL,
    subgraph_slug       TEXT NOT NULL,
    schema_version      INT,
    draft_id            UUID REFERENCES xb_schema_drafts(id) ON DELETE SET NULL,
    cert_level          TEXT NOT NULL
                        CHECK (cert_level IN ('L0_INVALID', 'L1_CONFIGURED', 'L2_RUNNABLE', 'L3_VERIFIED', 'L4_CERTIFIED')),
    readiness_score     INT NOT NULL CHECK (readiness_score BETWEEN 0 AND 100),
    dead_knowledge_cnt  INT NOT NULL DEFAULT 0,
    high_fanout_cnt     INT NOT NULL DEFAULT 0,
    trial_tick_passed   BOOLEAN NOT NULL DEFAULT FALSE,
    triggered_rules_cnt INT NOT NULL DEFAULT 0,
    report_json         JSONB NOT NULL DEFAULT '{}'::jsonb,
    certified_by        UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_runtime_cert_subgraph
    ON xb_runtime_certifications (tenant_id, subgraph_slug, created_at DESC);

SELECT ax_enable_tenant_rls('xb_runtime_certifications');

ALTER TABLE ax_approvals DROP CONSTRAINT IF EXISTS ax_approvals_source_kind_check;
ALTER TABLE ax_approvals ADD CONSTRAINT ax_approvals_source_kind_check
    CHECK (source_kind IN ('FLOW', 'SIM_APPLY', 'AGENT', 'GRANT', 'OBJECT_ACTION'));

ALTER TABLE ax_action_log DROP CONSTRAINT IF EXISTS ax_action_log_source_kind_check;
ALTER TABLE ax_action_log ADD CONSTRAINT ax_action_log_source_kind_check
    CHECK (source_kind IN ('FLOW', 'SIM_APPLY', 'AGENT', 'RULE', 'VIEW', 'OBJECT_ACTION'));

ALTER TABLE ax_action_log DROP CONSTRAINT IF EXISTS ax_action_log_action_kind_check;
ALTER TABLE ax_action_log ADD CONSTRAINT ax_action_log_action_kind_check
    CHECK (action_kind IN ('WRITE_NODE_METRIC', 'WRITE_NODE_PROPERTY', 'HTTP', 'NOTIFY', 'WORK_ITEM', 'OBJECT_ACTION'));

ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE','STUDIO_SNAPSHOT_RESTORE','IM_ACCOUNT_BIND',
        'CHART_PUBLISH','OBJECT_ACTION_INVOKE','TRIAL_TICK_CERTIFY'
    ));
