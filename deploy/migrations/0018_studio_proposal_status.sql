-- 0018_studio_proposal_status.sql：模型草案版本快照、校验通过项计数、节点语义分层与快照回退审计

-- 1. 模型草案增量快照表：每次 AI 改写或批量确认前自动写入回退点
CREATE TABLE IF NOT EXISTS xb_schema_draft_snapshots (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    draft_id    UUID NOT NULL REFERENCES xb_schema_drafts(id) ON DELETE CASCADE,
    snapshot_no INT NOT NULL CHECK (snapshot_no > 0),
    reason      TEXT NOT NULL DEFAULT 'COPILOT_PATCH'
                CHECK (reason IN ('MANUAL', 'COPILOT_PATCH', 'BATCH_CONFIRM', 'PRE_MERGE')),
    schema_doc  JSONB NOT NULL,
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (draft_id, snapshot_no)
);

CREATE INDEX IF NOT EXISTS idx_xb_draft_snapshots_lookup
    ON xb_schema_draft_snapshots (tenant_id, draft_id, snapshot_no DESC);

SELECT ax_enable_tenant_rls('xb_schema_draft_snapshots');
GRANT SELECT, INSERT, DELETE ON xb_schema_draft_snapshots TO axpoint_app;

-- 2. 扩展模型草案：记录正向静态校验通过项数量
ALTER TABLE xb_schema_drafts
    ADD COLUMN IF NOT EXISTS pass_count INT NOT NULL DEFAULT 0 CHECK (pass_count >= 0);

-- 3. 扩展已发布节点类型：可选语义层级与业务域分组
ALTER TABLE xb_node_types
    ADD COLUMN IF NOT EXISTS semantic_layer_tier TEXT
        CHECK (semantic_layer_tier IN ('STATIC_FACT', 'EVENT_DYNAMIC', 'CUBE_ROLLUP'));

ALTER TABLE xb_node_types
    ADD COLUMN IF NOT EXISTS domain_group TEXT;

-- 4. 扩展审计动作枚举：追加 STUDIO_SNAPSHOT_RESTORE
ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE','STUDIO_SNAPSHOT_RESTORE'
    ));
