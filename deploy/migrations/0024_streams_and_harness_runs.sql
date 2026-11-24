-- 0024_streams_and_harness_runs.sql：常驻流消费租约、LOG_CDC 扩展与能力问答验收套件表

ALTER TABLE ds_datasets
    DROP CONSTRAINT IF EXISTS ds_datasets_sync_mode_check;

ALTER TABLE ds_datasets
    ADD CONSTRAINT ds_datasets_sync_mode_check
        CHECK (sync_mode IN ('SNAPSHOT', 'APPEND', 'UPSERT', 'LOG_CDC'));

ALTER TABLE ds_datasets
    ADD COLUMN IF NOT EXISTS continuous_stream BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS cdc_slot_meta JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS stream_lease_holder TEXT,
    ADD COLUMN IF NOT EXISTS stream_lease_until TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS xb_competency_suites (
    tenant_id       TEXT NOT NULL,
    subgraph_slug   TEXT NOT NULL,
    questions       JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_by      UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, subgraph_slug)
);

SELECT ax_enable_tenant_rls('xb_competency_suites');

CREATE TABLE IF NOT EXISTS xb_harness_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    subgraph_slug   TEXT NOT NULL,
    total_questions INT NOT NULL DEFAULT 0,
    passed_count    INT NOT NULL DEFAULT 0,
    status          TEXT NOT NULL CHECK (status IN ('PASS', 'GAP_DETECTED', 'FAILED')),
    gap_diagnostics JSONB NOT NULL DEFAULT '[]'::jsonb,
    results_json    JSONB NOT NULL DEFAULT '[]'::jsonb,
    triggered_by    UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_harness_runs_lookup
    ON xb_harness_runs (tenant_id, subgraph_slug, created_at DESC);

SELECT ax_enable_tenant_rls('xb_harness_runs');

-- 扩展审计动作枚举支持 HARNESS_VERIFY_RUN
ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE','STUDIO_SNAPSHOT_RESTORE','IM_ACCOUNT_BIND',
        'CHART_PUBLISH','OBJECT_ACTION_INVOKE','TRIAL_TICK_CERTIFY','HARNESS_VERIFY_RUN'
    ));

-- 跨租户常驻流消费数据集单主租约认领与续期
CREATE OR REPLACE FUNCTION ax_claim_stream_datasets(
    p_holder TEXT,
    p_limit INT,
    p_lease_seconds INT
)
RETURNS TABLE (
    tenant_id  TEXT,
    dataset_id UUID
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public
AS $$
    UPDATE ds_datasets d
       SET stream_lease_holder = p_holder,
           stream_lease_until  = NOW() + make_interval(secs => p_lease_seconds),
           updated_at          = NOW()
     WHERE d.id IN (
           SELECT id
             FROM ds_datasets
            WHERE continuous_stream = TRUE
              AND status != 'DISABLED'
              AND connection_id IS NOT NULL
              AND (stream_lease_holder = p_holder
                   OR stream_lease_until IS NULL
                   OR stream_lease_until < NOW())
            ORDER BY CASE WHEN stream_lease_holder = p_holder THEN 0 ELSE 1 END,
                     stream_lease_until NULLS FIRST,
                     id
            FOR UPDATE SKIP LOCKED
            LIMIT p_limit
     )
    RETURNING d.tenant_id, d.id;
$$;

-- 优雅停机主动释放本实例持有的常驻流消费租约
CREATE OR REPLACE FUNCTION ax_release_stream_datasets(p_holder TEXT)
RETURNS INT
LANGUAGE sql
SECURITY DEFINER
SET search_path = public
AS $$
    WITH released AS (
        UPDATE ds_datasets
           SET stream_lease_holder = NULL,
               stream_lease_until  = NULL,
               updated_at          = NOW()
         WHERE stream_lease_holder = p_holder
        RETURNING 1
    )
    SELECT count(*)::INT FROM released;
$$;

REVOKE ALL ON FUNCTION ax_claim_stream_datasets(TEXT, INT, INT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_claim_stream_datasets(TEXT, INT, INT) TO axpoint_app;

REVOKE ALL ON FUNCTION ax_release_stream_datasets(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_release_stream_datasets(TEXT) TO axpoint_app;
