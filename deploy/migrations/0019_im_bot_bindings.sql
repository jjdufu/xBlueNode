-- 0019_im_bot_bindings.sql：企业 IM 出站长连接机器人、用户身份绑定、挑战码与分布式租约

CREATE TABLE IF NOT EXISTS ax_im_connectors (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          TEXT NOT NULL,
    name               TEXT NOT NULL,
    platform           TEXT NOT NULL CHECK (platform IN ('FEISHU','DINGTALK','WECOM')),
    agent_id           UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    credential_id      UUID NOT NULL REFERENCES ds_credentials(id) ON DELETE RESTRICT,
    config             JSONB NOT NULL DEFAULT '{}'::jsonb,
    identity_policy    TEXT NOT NULL DEFAULT 'REQUIRE_USER_BIND'
                           CHECK (identity_policy IN ('REQUIRE_USER_BIND','CREATOR_DELEGATED')),
    delegated_user_id  UUID REFERENCES sec_users(id) ON DELETE RESTRICT,
    enabled            BOOLEAN NOT NULL DEFAULT TRUE,
    leader_holder      TEXT,
    lease_until        TIMESTAMPTZ,
    status             TEXT NOT NULL DEFAULT 'DISCONNECTED'
                           CHECK (status IN ('DISCONNECTED','CONNECTING','CONNECTED','DEGRADED','ERROR')),
    last_error         TEXT NOT NULL DEFAULT '',
    last_heartbeat_at  TIMESTAMPTZ,
    created_by         UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name),
    CHECK (identity_policy <> 'CREATOR_DELEGATED' OR delegated_user_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_ax_im_connectors_lease
    ON ax_im_connectors (enabled, lease_until);

CREATE TABLE IF NOT EXISTS ax_im_user_bindings (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             TEXT NOT NULL,
    platform              TEXT NOT NULL CHECK (platform IN ('FEISHU','DINGTALK','WECOM')),
    external_user_id      TEXT NOT NULL,
    external_display_name TEXT NOT NULL DEFAULT '',
    user_id               UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    bound_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, platform, external_user_id)
);
CREATE INDEX IF NOT EXISTS idx_ax_im_user_bindings_user
    ON ax_im_user_bindings (tenant_id, user_id);

CREATE TABLE IF NOT EXISTS ax_im_bind_challenges (
    token_hash            TEXT PRIMARY KEY,
    tenant_id             TEXT NOT NULL,
    connector_id          UUID NOT NULL REFERENCES ax_im_connectors(id) ON DELETE CASCADE,
    platform              TEXT NOT NULL CHECK (platform IN ('FEISHU','DINGTALK','WECOM')),
    external_user_id      TEXT NOT NULL,
    external_display_name TEXT NOT NULL DEFAULT '',
    expires_at            TIMESTAMPTZ NOT NULL,
    consumed_at           TIMESTAMPTZ,
    consumed_by           UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_im_bind_challenges_exp
    ON ax_im_bind_challenges (tenant_id, expires_at);

CREATE TABLE IF NOT EXISTS ax_im_event_dedup (
    tenant_id    TEXT NOT NULL,
    connector_id UUID NOT NULL REFERENCES ax_im_connectors(id) ON DELETE CASCADE,
    event_id     TEXT NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (connector_id, event_id)
);
CREATE INDEX IF NOT EXISTS idx_ax_im_event_dedup_time
    ON ax_im_event_dedup (tenant_id, received_at);

-- 扩展 ax_conversations 支持 IM 通道与外部会话线程续接
ALTER TABLE ax_conversations DROP CONSTRAINT IF EXISTS ax_conversations_channel_check;
ALTER TABLE ax_conversations ADD CONSTRAINT ax_conversations_channel_check
    CHECK (channel IN ('WORKBENCH','EMBED','API','FLOW','MCP','IM'));
ALTER TABLE ax_conversations ADD COLUMN IF NOT EXISTS im_connector_id UUID REFERENCES ax_im_connectors(id) ON DELETE SET NULL;
ALTER TABLE ax_conversations ADD COLUMN IF NOT EXISTS external_chat_id TEXT;
CREATE INDEX IF NOT EXISTS idx_ax_conversations_im_thread
    ON ax_conversations (tenant_id, im_connector_id, external_chat_id, user_id, updated_at DESC)
    WHERE channel = 'IM';

-- 扩展保留策略目标枚举支持清理过期 IM 事件去重表与挑战码
ALTER TABLE ax_retention_policies DROP CONSTRAINT IF EXISTS ax_retention_policies_target_check;
ALTER TABLE ax_retention_policies ADD CONSTRAINT ax_retention_policies_target_check
    CHECK (target IN (
        'METRIC_SERIES','METRIC_ROLLUP_MINUTE','AUDIT','EXPORT_FILE','FLOW_STEP_LOG','JOB',
        'OUTBOX','LAKE_SNAPSHOT','MESSAGE','QUERY_HISTORY','CONVERSATION','IM_EVENT_DEDUP'
    ));
INSERT INTO ax_retention_policies (tenant_id, target, keep_days)
VALUES ('default', 'IM_EVENT_DEDUP', 7)
ON CONFLICT (tenant_id, target) DO NOTHING;

-- 扩展审计动作枚举支持 IM_ACCOUNT_BIND
ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE','STUDIO_SNAPSHOT_RESTORE','IM_ACCOUNT_BIND'
    ));

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_im_connectors',
    'ax_im_user_bindings',
    'ax_im_bind_challenges',
    'ax_im_event_dedup'
]::regclass[]) AS t;

-- 跨租户后台连接器租约认领与续期（单主防脑裂）
CREATE OR REPLACE FUNCTION ax_claim_im_connectors(
    p_holder TEXT,
    p_limit INT,
    p_lease_seconds INT
)
RETURNS SETOF ax_im_connectors
LANGUAGE sql
SECURITY DEFINER
SET search_path = public
AS $$
    UPDATE ax_im_connectors c
       SET leader_holder     = p_holder,
           lease_until       = NOW() + make_interval(secs => p_lease_seconds),
           last_heartbeat_at = NOW(),
           updated_at        = NOW()
     WHERE c.id IN (
           SELECT id
             FROM ax_im_connectors
            WHERE enabled = TRUE
              AND (leader_holder = p_holder
                   OR lease_until IS NULL
                   OR lease_until < NOW())
            ORDER BY CASE WHEN leader_holder = p_holder THEN 0 ELSE 1 END,
                     lease_until NULLS FIRST,
                     id
            FOR UPDATE SKIP LOCKED
            LIMIT p_limit
     )
    RETURNING c.*;
$$;

-- 优雅停机时主动归还本实例持有的全部 IM 租约
CREATE OR REPLACE FUNCTION ax_release_im_connectors(p_holder TEXT)
RETURNS INT
LANGUAGE sql
SECURITY DEFINER
SET search_path = public
AS $$
    WITH released AS (
        UPDATE ax_im_connectors
           SET leader_holder = NULL,
               lease_until   = NULL,
               status        = 'DISCONNECTED',
               updated_at    = NOW()
         WHERE leader_holder = p_holder
        RETURNING 1
    )
    SELECT count(*)::INT FROM released;
$$;

-- 原子校验并核销一次性 IM 绑定挑战码，同时写入或更新账号绑定表
CREATE OR REPLACE FUNCTION ax_consume_im_bind_challenge(
    p_token_hash TEXT,
    p_tenant_id  TEXT,
    p_user_id    UUID
)
RETURNS TABLE (
    connector_id          UUID,
    platform              TEXT,
    external_user_id      TEXT,
    external_display_name TEXT
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    v_row ax_im_bind_challenges%ROWTYPE;
BEGIN
    UPDATE ax_im_bind_challenges ch
       SET consumed_at = NOW(),
           consumed_by = p_user_id
     WHERE ch.token_hash  = p_token_hash
       AND ch.tenant_id   = p_tenant_id
       AND ch.consumed_at IS NULL
       AND ch.expires_at  > NOW()
    RETURNING * INTO v_row;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    INSERT INTO ax_im_user_bindings (
        tenant_id, platform, external_user_id, external_display_name, user_id, bound_at
    )
    VALUES (
        v_row.tenant_id, v_row.platform, v_row.external_user_id,
        v_row.external_display_name, p_user_id, NOW()
    )
    ON CONFLICT (tenant_id, platform, external_user_id)
    DO UPDATE SET
        user_id               = EXCLUDED.user_id,
        external_display_name = EXCLUDED.external_display_name,
        bound_at              = NOW();

    RETURN QUERY
    SELECT v_row.connector_id, v_row.platform, v_row.external_user_id, v_row.external_display_name;
END;
$$;

REVOKE ALL ON FUNCTION ax_claim_im_connectors(TEXT, INT, INT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_release_im_connectors(TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_consume_im_bind_challenge(TEXT, TEXT, UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_claim_im_connectors(TEXT, INT, INT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_release_im_connectors(TEXT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_consume_im_bind_challenge(TEXT, TEXT, UUID) TO axpoint_app;
